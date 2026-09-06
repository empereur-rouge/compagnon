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
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::db::{Base, ErreurBase};
use crate::horloge;
use crate::modele::ClientModele;
use crate::personnage::sceau::Sceau;
use crate::http::{self, EtatApp};
use crate::scrutation;
use crate::telegram::envoi::ErreurEnvoi;
use crate::telegram::{Canal, ErreurCanal};
use crate::worker::Equipe;

/// Délai au-delà duquel on cesse d'attendre les tâches en cours, à l'extinction.
///
/// Ne « vide » plus rien, malgré son nom d'origine : ce qui reste en file survit à l'arrêt.
/// Cette borne empêche seulement un worker bloqué de retenir le processus.
///
/// **Ce nombre est lié à `stop_grace_period` dans `compose.yaml`** : Docker envoie `SIGKILL`
/// passé son propre sursis, et un vidage plus long que lui serait tranché sans un mot. Les deux
/// valeurs doivent bouger ensemble ; celle-ci est prise en dessous pour que l'abandon soit
/// journalisé par le service plutôt que constaté par son absence.
const DELAI_VIDAGE: Duration = Duration::from_secs(25);

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

    /// La base est injoignable, ou son schéma n'a pas pu être mis à jour.
    ///
    /// Empêche le démarrage, au même titre qu'un jeton refusé : un service qui accepte des
    /// requêtes pour toutes les mettre en échec est pire qu'un service qui refuse de partir.
    #[error("base de données indisponible : {0}")]
    Base(#[from] ErreurBase),
}

/// Un service assemblé et lié, prêt à servir.
///
/// L'existence de cet état intermédiaire n'est pas une commodité de test : elle rend
/// l'**adresse effective** connaissable avant que le service ne parte. Sans elle, un test lié
/// sur le port `0` n'aurait aucun moyen de savoir où frapper.
pub struct Prepare {
    /// L'adresse réellement obtenue, port éphémère résolu.
    pub adresse: SocketAddr,
    ecoute: TcpListener,
    routeur: Router,
    equipe: Equipe,
}

/// Assemble le service et prend l'adresse d'écoute, sans encore servir.
///
/// Le client de modèle est **reçu**, pas construit ici : c'est ce qui permet aux tests
/// d'injecter un double et d'éprouver le service entier face à un fournisseur qui expire,
/// refuse, ou ne rend rien. Le binaire lui passe l'implémentation HTTP.
///
/// # Errors
///
/// Renvoie [`ErreurDemarrage`] si le canal ne se construit pas, si Telegram refuse le jeton,
/// ou si l'adresse d'écoute est indisponible.
pub async fn preparer(
    config: &Config,
    modele: Arc<dyn ClientModele>,
    sceau: Arc<Sceau>,
) -> Result<Prepare, ErreurDemarrage> {
    let canal = Arc::new(Canal::new(config)?);

    let identite = canal.identite().await?;
    tracing::info!(
        bot_id = identite.id,
        nom = %identite.first_name,
        utilisateur = identite.username.as_deref().unwrap_or("(sans nom d'utilisateur)"),
        "jeton validé par Telegram"
    );

    // La base est jointe et migrée avant que quoi que ce soit ne démarre. Migrer ici plutôt
    // que par une commande séparée retire une étape d'exploitation qu'on peut oublier — et un
    // service tournant sur un schéma incomplet est une panne qui ne se déclare qu'au premier
    // message, donc devant un utilisateur.
    let base = Base::ouvrir(config.url_base.exposer()).await?;
    tracing::info!(base = %crate::config::masquer_url(config.url_base.exposer()), "base jointe et migrée");

    // L'écoute est prise AVANT le lancement du worker : tout ce qui peut échouer d'abord,
    // tout ce qui démarre ensuite. Dans l'ordre inverse, un `bind` refusé laissait une tâche
    // détachée derrière lui — elle sortait d'elle-même, mais par la mécanique des `drop`
    // plutôt que par intention, et il fallait le démontrer pour s'en convaincre.
    let echec = |source| ErreurDemarrage::Ecoute {
        adresse: config.adresse_ecoute,
        source,
    };
    let ecoute = TcpListener::bind(config.adresse_ecoute)
        .await
        .map_err(echec)?;
    // L'adresse effective, et non celle demandée : lier sur le port zéro donne un port
    // éphémère que seul le système connaît, et c'est celui-là qu'il faut annoncer.
    let adresse = ecoute.local_addr().map_err(echec)?;

    let equipe = Equipe::lancer(&base, &canal, &modele, &sceau);

    let routeur = http::routeur(EtatApp {
        canal,
        base,
        workers_vivants: equipe.vivants(),
        demarre_le: horloge::maintenant(),
    });

    Ok(Prepare {
        adresse,
        ecoute,
        routeur,
        equipe,
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
            equipe,
        } = self;

        tracing::info!(%adresse, version = crate::VERSION, "service à l'écoute");

        // Le routeur est consommé par `serve`, et relâché à la fin de cette instruction. C'est
        // ce relâchement qui ferme l'entrée de la file — donc l'ordre des deux lignes qui
        // suivent n'est pas indifférent : attendre le worker avant que `serve` ait rendu la
        // main attendrait indéfiniment.
        let resultat = axum::serve(ecoute, routeur)
            .with_graceful_shutdown(arret)
            .await;

        // Ce que l'extinction doit garantir a changé avec la file en base : il ne s'agit plus
        // de la vider — ce qu'elle contient survit à l'arrêt et sera repris au démarrage
        // suivant — mais seulement de laisser les tâches EN COURS se terminer. Une tâche
        // interrompue serait reprise au bail, et l'utilisateur recevrait deux fois la même
        // réponse.
        equipe.eteindre(DELAI_VIDAGE).await;

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
    modele: Arc<dyn ClientModele>,
    sceau: Arc<Sceau>,
    arret: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ErreurDemarrage> {
    preparer(config, modele, sceau).await?.servir(arret).await
}

/// Écoute Telegram par scrutation, sans servir de webhook.
///
/// # Ce que cette fonction change, et ce qu'elle ne change pas
///
/// Elle change la **porte d'entrée**, et rien d'autre : pas de socket d'écoute, pas de
/// routeur, pas de TLS, donc rien à exposer sur Internet. Tout ce qui suit — l'admission, la
/// file, le worker, l'extinction — est le code de production, appelé tel quel. C'est la
/// condition pour qu'un comportement observé ici veuille dire quelque chose.
///
/// Le webhook est **retiré** au démarrage : Telegram interdit de mêler les deux modes et
/// répondrait `409` à chaque appel. Le retrait est donc un geste délibéré et journalisé, pas
/// un effet de bord — un développeur qui scrute sur le jeton de production coupe sa
/// production, et doit le lire dans les journaux plutôt que le découvrir.
///
/// # Errors
///
/// Renvoie [`ErreurDemarrage`] si le canal ne se construit pas ou si Telegram refuse le jeton.
pub async fn scruter(
    config: &Config,
    modele: Arc<dyn ClientModele>,
    sceau: Arc<Sceau>,
    arret: impl Future<Output = ()> + Send,
) -> Result<(), ErreurDemarrage> {
    let canal = Canal::new(config)?;

    let identite = canal.identite().await?;
    let nom = identite
        .username
        .map_or_else(|| identite.first_name.clone(), |u| format!("@{u}"));
    tracing::info!(bot_id = identite.id, bot = %nom, "jeton validé par Telegram");

    // Sans ce retrait, tous les appels suivants échoueraient en `409`, et le message de
    // Telegram ne nommerait pas la cause.
    canal.retirer_webhook().await?;
    tracing::info!("webhook retiré : la scrutation et le webhook s'excluent");

    let base = Base::ouvrir(config.url_base.exposer()).await?;
    tracing::info!(base = %crate::config::masquer_url(config.url_base.exposer()), "base jointe et migrée");

    let canal = Arc::new(canal);
    let equipe = Equipe::lancer(&base, &canal, &modele, &sceau);

    scrutation::tourner(&canal, &base, arret).await;

    // Même contrat d'extinction que le service webhook, et désormais le même code : ce qui
    // reste en file survit à l'arrêt, seules les tâches en cours doivent finir.
    equipe.eteindre(DELAI_VIDAGE).await;
    Ok(())
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
