//! Séquence de démarrage, partagée par la production et les tests.
//!
//! # Pourquoi elle n'est pas dans `main.rs`
//!
//! Un test qui construirait son propre assemblage testerait cet assemblage-là, pas celui qui
//! tourne en production. Les deux chemins passent donc ici. `main.rs` ne garde que ce qui
//! appartient à un binaire : les arguments, les signaux, le code de sortie.
//!
//! # Pourquoi `getMe` avant d'écouter
//!
//! Un jeton révoqué, mal collé ou appartenant à un autre bot ne se voit pas autrement. Sans cet
//! appel, le service démarrerait, répondrait `ok` à sa propre sonde, accepterait des mises à
//! jour — et échouerait silencieusement à chaque réponse. L'ordre choisi fait échouer le
//! **démarrage**, ce qu'un redéploiement rend immédiatement visible.
//!
//! # Extinction
//!
//! [`Prepare::servir`] rend la main quand, dans l'ordre : le serveur a fini de servir les
//! requêtes en cours, le routeur a été relâché — donc l'entrée de la file aussi — et le worker
//! a vidé ce qui restait. Un message accepté est donc traité, même arrivé à la seconde qui
//! précède l'arrêt.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::horloge;
use crate::http::{self, EtatApp};
use crate::telegram::envoi::{ErreurEnvoi, Identite};
use crate::telegram::{Canal, ErreurCanal};
use crate::worker::{self, CAPACITE_FILE};

/// Ce qui a empêché le service de démarrer ou de servir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurDemarrage {
    /// Le canal Telegram n'a pas pu être construit.
    #[error("canal Telegram inconstructible : {0}")]
    Canal(#[from] ErreurCanal),

    /// Telegram n'a pas reconnu le jeton.
    ///
    /// Le message ne cite pas le jeton — il ne cite jamais que la méthode appelée.
    #[error("jeton refusé par Telegram : {0}")]
    JetonRefuse(#[from] ErreurEnvoi),

    /// L'adresse d'écoute n'a pas pu être prise.
    #[error("écoute impossible sur {adresse} : {source}")]
    Ecoute {
        /// L'adresse demandée.
        adresse: SocketAddr,
        /// La cause système.
        source: std::io::Error,
    },

    /// Le serveur s'est arrêté sur une erreur.
    #[error("le service s'est interrompu : {0}")]
    Service(std::io::Error),
}

/// Un service assemblé et lié, prêt à servir.
///
/// L'existence de cet état intermédiaire n'est pas une commodité de test : elle rend
/// l'**adresse effective** connaissable avant que le service ne parte. Sans elle, un test lié
/// sur le port `0` n'aurait aucun moyen de savoir où frapper.
pub struct Prepare {
    /// L'adresse réellement obtenue, port éphémère résolu.
    pub adresse: SocketAddr,
    /// Qui Telegram dit que ce bot est.
    pub identite: Identite,
    ecoute: TcpListener,
    routeur: Router,
    worker: JoinHandle<()>,
}

/// Assemble le service et prend l'adresse d'écoute, sans encore servir.
///
/// # Errors
///
/// Renvoie [`ErreurDemarrage`] si le canal ne se construit pas, si Telegram refuse le jeton,
/// ou si l'adresse d'écoute est indisponible.
pub async fn preparer(config: &Config) -> Result<Prepare, ErreurDemarrage> {
    let canal = Arc::new(Canal::new(config)?);

    let identite = canal.identite().await?;
    tracing::info!(
        bot_id = identite.id,
        nom = %identite.first_name,
        utilisateur = identite.username.as_deref().unwrap_or("(sans nom d'utilisateur)"),
        "jeton validé par Telegram"
    );

    let (expediteur, reception) = mpsc::channel(CAPACITE_FILE);
    let worker = tokio::spawn(worker::tourner(reception, Arc::clone(&canal)));

    let etat = EtatApp::new(canal, expediteur, horloge::maintenant());
    let routeur = http::routeur(etat);

    let ecoute = TcpListener::bind(config.adresse_ecoute)
        .await
        .map_err(|source| ErreurDemarrage::Ecoute {
            adresse: config.adresse_ecoute,
            source,
        })?;
    let adresse = http::adresse_liee(&ecoute).map_err(|source| ErreurDemarrage::Ecoute {
        adresse: config.adresse_ecoute,
        source,
    })?;

    Ok(Prepare {
        adresse,
        identite,
        ecoute,
        routeur,
        worker,
    })
}

impl Prepare {
    /// Sert jusqu'à ce que `arret` se réalise, puis vide la file avant de rendre la main.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurDemarrage::Service`] si le serveur s'interrompt sur une erreur système.
    pub async fn servir(
        self,
        arret: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ErreurDemarrage> {
        let Self {
            adresse,
            ecoute,
            routeur,
            worker,
            ..
        } = self;

        tracing::info!(%adresse, version = crate::VERSION, "service à l'écoute");

        // Le routeur est consommé par `serve`, et relâché à la fin de cette instruction. C'est
        // ce relâchement qui ferme l'entrée de la file — donc l'ordre des deux lignes qui
        // suivent n'est pas indifférent : attendre le worker avant que `serve` ait rendu la
        // main attendrait indéfiniment.
        let resultat = axum::serve(ecoute, routeur)
            .with_graceful_shutdown(arret)
            .await;

        tracing::info!("plus de requête en cours, vidage de la file");
        if let Err(erreur) = worker.await {
            tracing::error!(%erreur, "le worker s'est interrompu anormalement");
        }

        resultat.map_err(ErreurDemarrage::Service)
    }
}

/// Démarre le service et rend la main quand il s'est arrêté.
///
/// # Errors
///
/// Renvoie [`ErreurDemarrage`] à la première étape qui échoue.
pub async fn servir(
    config: &Config,
    arret: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ErreurDemarrage> {
    preparer(config).await?.servir(arret).await
}

/// Se réalise au premier `SIGINT` ou `SIGTERM`.
///
/// `SIGTERM` est celui que `docker stop` envoie ; sans lui, l'arrêt d'un conteneur serait une
/// coupure brutale au bout du délai de grâce, et la file en mémoire y perdrait son contenu.
///
/// # Panics
///
/// Ne panique pas : si un gestionnaire de signal ne peut être installé, la fonction se contente
/// d'attendre l'autre.
pub async fn signal_d_arret() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            // Ne jamais rendre la main immédiatement : cela déclencherait une extinction
            // instantanée au démarrage.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminaison = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut flux) => {
                flux.recv().await;
            }
            Err(erreur) => {
                tracing::warn!(%erreur, "SIGTERM non écouté, seul Ctrl-C arrêtera le service");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminaison = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!(signal = "SIGINT", "arrêt demandé"),
        () = terminaison => tracing::info!(signal = "SIGTERM", "arrêt demandé"),
    }
}
