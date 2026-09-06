//! Point d'entrée du service `compagnon`.
//!
//! Volontairement mince : la séquence de démarrage vit dans [`compagnon::app`] et les gestes
//! d'exploitation dans [`compagnon::cli`], pour que les tests exercent exactement le chemin de
//! la production. Ne restent ici que la journalisation, la lecture des arguments et le code de
//! sortie — ce qui appartient à un binaire.
//!
//! Les commandes sont listées dans [`USAGE`], et à un seul endroit : les deux listes qui
//! coexistaient ici avaient **déjà divergé** — l'une ignorait `compagnon activer` et
//! `compagnon verifier`, l'autre `catalogues`, `compagnon creer/montrer` et `utilisateur age`.
//! Une liste qui se maintient à deux endroits n'est à jour nulle part.

use std::sync::Arc;

use compagnon::config::Config;
use compagnon::modele::ClientModele;
use compagnon::modele::http::{ClientHttp, ConfigModele};
use compagnon::personnage::sceau::Sceau;
use compagnon::{VERSION, app, cli, cli_compagnon, cli_modele, telemetry};

/// Code de sortie quand le service refuse de démarrer, ou qu'une commande échoue.
const SORTIE_ERREUR: i32 = 1;

/// Code de sortie d'un usage incorrect, distinct d'un échec d'exécution.
const SORTIE_USAGE: i32 = 2;

/// Ce que `compagnon --help` affiche.
const USAGE: &str = "\
compagnon — plateforme de personnages conversationnels sur Telegram

  compagnon                          sert le webhook et fait tourner le worker
  compagnon ecouter                  reçoit par scrutation — ni domaine, ni TLS, ni tunnel ;
                                     pour éprouver le bot depuis un poste de travail
  compagnon sonde                    interroge /health, sort en 0 ou 1

  compagnon catalogues               ce parmi quoi un compagnon peut être composé
  compagnon compagnon creer …        crée un compagnon à partir de choix de catalogue
  compagnon compagnon montrer <id>   affiche le prompt composé et son empreinte
  compagnon compagnon verifier <id>  passe le compagnon par la modération
  compagnon compagnon activer <id>   active un compagnon validé
  compagnon utilisateur age <id>     enregistre une vérification d'âge

  compagnon modele essai <texte>     appelle le fournisseur configuré pour de vrai : réponse,
                                     jetons, durée mesurée et coût au tarif en vigueur

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
        ["ecouter"] => {
            telemetry::init();
            ecouter().await;
        }
        ["sonde"] => {
            telemetry::init_vers_stderr();
            rendre_compte(cli::sonde(&config_ou_sortir()).await);
        }
        ["webhook", "declarer", url] => {
            telemetry::init_vers_stderr();
            rendre_compte(cli::declarer_webhook(&config_ou_sortir(), url).await);
        }
        ["catalogues"] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(cli_compagnon::montrer_catalogues(&config_ou_sortir()).await);
        }
        ["compagnon", "creer", reste @ ..] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(cli_compagnon::creer(&config_ou_sortir(), reste).await);
        }
        ["compagnon", "montrer", utilisateur] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(cli_compagnon::montrer(&config_ou_sortir(), utilisateur).await);
        }
        ["compagnon", "activer", utilisateur] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(cli_compagnon::activer(&config_ou_sortir(), utilisateur).await);
        }
        ["compagnon", "verifier", utilisateur] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(
                cli_compagnon::verifier(&config_ou_sortir(), utilisateur).await,
            );
        }
        ["utilisateur", "age", utilisateur] => {
            telemetry::init_vers_stderr();
            rendre_compte_compagnon(
                cli_compagnon::verifier_age(&config_ou_sortir(), utilisateur).await,
            );
        }
        ["modele", "essai", reste @ ..] if !reste.is_empty() => {
            telemetry::init_vers_stderr();
            if let Err(erreur) = cli_modele::essai(&reste.join(" ")).await {
                eprintln!("{erreur}");
                std::process::exit(SORTIE_ERREUR);
            }
        }
        ["webhook", "retirer"] => {
            telemetry::init_vers_stderr();
            rendre_compte(cli::retirer_webhook(&config_ou_sortir()).await);
        }
        _ => {
            eprint!("{USAGE}");
            std::process::exit(SORTIE_USAGE);
        }
    }
}

/// Lit la clé de scellement du prompt, ou sort.
///
/// Séparé de [`config_ou_sortir`] parce que les deux configurations n'ont pas la même portée :
/// les commandes d'exploitation — sonde, webhook — n'ont aucune raison d'exiger une clé de
/// fournisseur, et la leur imposer bloquerait un diagnostic au moment où on en a besoin.
///
/// En revanche, `servir` et `ecouter` l'exigent, et l'exigent **au démarrage**. Un service qui
/// part sans modèle ne se découvre qu'au premier message, c'est-à-dire devant quelqu'un.
fn sceau_ou_sortir() -> Arc<Sceau> {
    match Sceau::depuis_environnement() {
        Ok(sceau) => Arc::new(sceau),
        Err(erreur) => {
            tracing::error!(%erreur, "clé de scellement du prompt refusée");
            std::process::exit(SORTIE_ERREUR);
        }
    }
}

/// Construit le client de modèle, ou sort en nommant ce qui manque.
fn modele_ou_sortir() -> Arc<dyn ClientModele> {
    let config = match ConfigModele::depuis_environnement() {
        Ok(config) => config,
        Err(erreur) => {
            tracing::error!(%erreur, "configuration du modèle refusée");
            std::process::exit(SORTIE_ERREUR);
        }
    };
    tracing::info!(
        fournisseur = %config.fournisseur,
        modele = %config.modele,
        jetons_max = config.jetons_max,
        delai = ?config.delai,
        "modèle configuré"
    );
    match ClientHttp::new(config) {
        Ok(client) => Arc::new(client),
        Err(erreur) => {
            tracing::error!(%erreur, "client de modèle impossible à construire");
            std::process::exit(SORTIE_ERREUR);
        }
    }
}

/// Charge la configuration, ou sort en nommant la variable fautive.
///
/// Un seul endroit pour cette politique. Elle a été écrite deux fois — une version journalisant
/// par `tracing::error!`, l'autre par `eprintln!` — alors que les commandes d'exploitation
/// viennent précisément d'envoyer `tracing` sur l'erreur standard : le second contournait le
/// mécanisme installé une ligne plus haut.
fn config_ou_sortir() -> Config {
    match Config::depuis_environnement() {
        Ok(config) => config,
        Err(erreur) => {
            tracing::error!(%erreur, "configuration refusée");
            std::process::exit(SORTIE_ERREUR);
        }
    }
}

/// Sort en erreur si une commande de compagnon a échoué.
///
/// Distincte de [`rendre_compte`] parce que les erreurs d'usage ne sont pas des pannes : elles
/// méritent le message tel quel, sans le décorum d'un journal d'incident, devant quelqu'un qui
/// vient de se tromper d'argument.
fn rendre_compte_compagnon(resultat: Result<(), cli_compagnon::ErreurCompagnon>) {
    if let Err(erreur) = resultat {
        eprintln!("{erreur}");
        std::process::exit(SORTIE_ERREUR);
    }
}

/// Sort en erreur si une commande d'exploitation a échoué.
fn rendre_compte(resultat: Result<(), cli::ErreurCli>) {
    if let Err(erreur) = resultat {
        tracing::error!(%erreur, "commande échouée");
        std::process::exit(SORTIE_ERREUR);
    }
}

/// Écoute par scrutation, et sort en erreur si le démarrage échoue.
async fn ecouter() {
    let config = config_ou_sortir();
    tracing::info!(config = ?config, "configuration chargée");

    if let Err(erreur) = app::scruter(&config, modele_ou_sortir(), sceau_ou_sortir(), app::signal_d_arret()).await {
        tracing::error!(%erreur, "la scrutation s'est arrêtée sur une erreur");
        std::process::exit(SORTIE_ERREUR);
    }
    tracing::info!("arrêt terminé");
}

/// Sert, et sort en erreur si le démarrage échoue.
async fn servir() {
    let config = config_ou_sortir();
    tracing::info!(config = ?config, "configuration chargée");

    if let Err(erreur) = app::servir(&config, modele_ou_sortir(), sceau_ou_sortir(), app::signal_d_arret()).await {
        tracing::error!(%erreur, "le service s'est arrêté sur une erreur");
        std::process::exit(SORTIE_ERREUR);
    }
    tracing::info!("arrêt terminé");
}
