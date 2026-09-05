//! Journalisation, initialisée une fois au démarrage.
//!
//! Le filtre par défaut est délibérément bavard sur `compagnon` et silencieux sur les crates
//! tierces : ce qu'un exploitant veut voir dans `docker compose logs`, c'est ce que *ce*
//! service a fait, pas la trace de connexion de `hyper`.

use tracing_subscriber::EnvFilter;

/// Filtre appliqué quand `RUST_LOG` n'est pas positionné.
const FILTRE_DEFAUT: &str = "compagnon=info,tower_http=warn,warn";

/// Installe la journalisation vers la sortie standard.
pub fn init() {
    installer(false);
}

/// Installe la journalisation vers l'erreur standard.
///
/// Réservé aux commandes d'exploitation dont la sortie standard porte un résultat destiné à
/// être redirigé — un JSON qu'on canalise vers `jq` ne doit pas être pollué par des lignes de
/// journal.
pub fn init_vers_stderr() {
    installer(true);
}

fn installer(vers_stderr: bool) {
    let filtre =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(FILTRE_DEFAUT));
    let constructeur = tracing_subscriber::fmt()
        .with_env_filter(filtre)
        .with_target(true)
        .with_level(true);

    // `try_init` et non `init` : un test qui appelle deux fois ne doit pas paniquer.
    let installe = if vers_stderr {
        constructeur.with_writer(std::io::stderr).try_init().is_ok()
    } else {
        constructeur.try_init().is_ok()
    };

    if installe {
        tracing::debug!(filtre_defaut = FILTRE_DEFAUT, "journalisation initialisée");
    }
}
