//! `compagnon` — plateforme de personnages conversationnels sur Telegram.
//!
//! # Ce que le service est
//!
//! Un bot Telegram unique derrière lequel vivent plusieurs personnages, écrits par les
//! utilisateurs. Chaque conversation lie une personne à un personnage ; le service tient la
//! mémoire de cette relation et rend au personnage une voix, un visage et une continuité.
//!
//! # Phase 0 — ce qui existe aujourd'hui
//!
//! La boucle nue, et rien d'autre : Telegram appelle le webhook, le service authentifie
//! l'appel, extrait le message, et répond en écho. Pas de base, pas de modèle, pas de
//! personnage. L'objet de cette phase est de **prouver le transport de bout en bout** — TLS,
//! secret partagé, découpage des messages, extinction propre — avant qu'une seule décision
//! produit ne repose dessus.
//!
//! Les phases suivantes remplacent l'écho, pas le transport :
//!
//! ```text
//! phase 1   base + moteur de dialogue + fiche de personnage
//! phase 2   mémoire (journal roulant, souvenirs structurés, état de relation)
//! phase 3   photos — file de génération à bail, cache de `file_id`
//! phase 4   audio — synthèse vocale sortante, transcription entrante
//! phase 5   génération d'images à la demande, ancre d'identité
//! phase 6   vidéo
//! ```
//!
//! # Où se trouve quoi
//!
//! | Module | Rôle |
//! |---|---|
//! | [`app`] | séquence de démarrage, partagée par la production et les tests |
//! | [`cli`] | gestes d'exploitation exécutables sur l'artefact livré |
//! | [`config`] | lecture et validation de l'environnement, au démarrage |
//! | [`error`] | codes d'erreur numériques stables de la surface HTTP |
//! | [`horloge`] | temps, en un seul endroit, pour que les tests puissent le figer |
//! | [`admission`] | ce qu'on retient d'une mise à jour, quelle que soit la porte |
//! | [`http`] | routeur, état partagé, sonde de santé |
//! | [`scrutation`] | réception sans domaine ni TLS, pour éprouver le bot en vrai |
//! | [`telegram`] | client de l'API Bot : authentification, envoi, découpage |
//! | [`webhook`] | réception des mises à jour |
//! | [`worker`] | file de traitement et production des réponses |

pub mod admission;
pub mod app;
pub mod cli;
pub mod config;
pub mod error;
pub mod horloge;
pub mod http;
pub mod scrutation;
pub mod telegram;
pub mod telemetry;
pub mod webhook;
pub mod worker;

/// Version du service, telle qu'elle apparaît dans `/health`, dans `--version` et dans les
/// journaux de démarrage.
///
/// Lue depuis `Cargo.toml` à la compilation : il n'existe pas de second endroit où la version
/// pourrait diverger.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
