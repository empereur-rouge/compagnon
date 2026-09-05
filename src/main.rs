//! Point d'entrée du service `compagnon`.
//!
//! Volontairement mince : la séquence de démarrage vit dans [`compagnon::app`] et les gestes
//! d'exploitation dans [`compagnon::cli`], pour que les tests exercent exactement le chemin de
//! la production. Ne restent ici que la journalisation, la lecture des arguments et le code de
//! sortie — ce qui appartient à un binaire.
//!
//! ```text
//! compagnon                          sert le webhook et fait tourner le worker
//! compagnon sonde                    interroge /health, sort en 0 ou 1 (HEALTHCHECK)
//! compagnon webhook declarer <url>   déclare l'adresse du webhook auprès de Telegram
//! compagnon webhook retirer          retire le webhook
//! ```

use compagnon::config::Config;
use compagnon::{VERSION, app, cli, telemetry};

/// Code de sortie quand le service refuse de démarrer, ou qu'une commande échoue.
const SORTIE_ERREUR: i32 = 1;

/// Code de sortie d'un usage incorrect, distinct d'un échec d'exécution.
const SORTIE_USAGE: i32 = 2;

/// Ce que `compagnon --help` affiche.
const USAGE: &str = "\
compagnon — plateforme de personnages conversationnels sur Telegram

  compagnon                          sert le webhook et fait tourner le worker
  compagnon sonde                    interroge /health, sort en 0 ou 1
  compagnon webhook declarer <url>   déclare l'adresse du webhook auprès de Telegram
  compagnon webhook retirer          retire le webhook

  -h, --help                         affiche cette aide
  -V, --version                      affiche la version

Configuration : voir .env.example. Toutes les variables sont validées au démarrage.
";

#[tokio::main]
async fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mots: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match mots.as_slice() {
        ["-h" | "--help"] => {
            print!("{USAGE}");
        }
        ["-V" | "--version"] => {
            println!("compagnon {VERSION}");
        }
        [] => {
            telemetry::init();
            servir().await;
        }
        // Les commandes d'exploitation journalisent vers l'erreur standard : leur sortie
        // standard porte un résultat qu'on redirige souvent vers `jq`.
        ["sonde"] => {
            telemetry::init_vers_stderr();
            executer(|config| async move { cli::sonde(&config).await }).await;
        }
        ["webhook", "declarer", url] => {
            telemetry::init_vers_stderr();
            let url = (*url).to_owned();
            executer(|config| async move { cli::declarer_webhook(&config, &url).await }).await;
        }
        ["webhook", "retirer"] => {
            telemetry::init_vers_stderr();
            executer(|config| async move { cli::retirer_webhook(&config).await }).await;
        }
        _ => {
            eprint!("{USAGE}");
            std::process::exit(SORTIE_USAGE);
        }
    }
}

/// Sert, et sort en erreur si le démarrage échoue.
async fn servir() {
    let config = match Config::depuis_environnement() {
        Ok(config) => config,
        Err(erreur) => {
            tracing::error!(%erreur, "configuration refusée, le service ne démarre pas");
            std::process::exit(SORTIE_ERREUR);
        }
    };
    tracing::info!(config = ?config, "configuration chargée");

    if let Err(erreur) = app::servir(&config, app::signal_d_arret()).await {
        tracing::error!(%erreur, "le service s'est arrêté sur une erreur");
        std::process::exit(SORTIE_ERREUR);
    }
    tracing::info!("arrêt terminé");
}

/// Exécute une commande d'exploitation avec la configuration chargée.
async fn executer<F, Fut>(commande: F)
where
    F: FnOnce(Config) -> Fut,
    Fut: Future<Output = Result<(), cli::ErreurCli>>,
{
    let config = match Config::depuis_environnement() {
        Ok(config) => config,
        Err(erreur) => {
            eprintln!("configuration refusée : {erreur}");
            std::process::exit(SORTIE_ERREUR);
        }
    };

    if let Err(erreur) = commande(config).await {
        eprintln!("échec : {erreur}");
        std::process::exit(SORTIE_ERREUR);
    }
}
