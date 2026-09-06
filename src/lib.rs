//! `compagnon` — plateforme de personnages conversationnels sur Telegram.
//!
//! # Ce que le service est
//!
//! Un bot Telegram unique, derrière lequel chaque utilisateur possède **son** assistant : il
//! l'a nommé, en a défini la personnalité, choisi l'apparence et la voix. Le service tient la
//! mémoire de cette relation et rend à l'assistant une voix, un visage et une continuité.
//!
//! Pas de catalogue partagé — voir `documentation/un-assistant-par-personne.md`.
//!
//! # Ce qui existe aujourd'hui
//!
//! Le circuit complet d'un message, jusqu'au modèle et retour. Telegram appelle le webhook (ou
//! le service scrute), le service authentifie, met en file, et un worker lit le compagnon de la
//! personne, envoie son prompt **validé** au modèle, renvoie la réponse, inscrit le fil et le
//! coût de l'appel.
//!
//! L'écho de la phase 0 a disparu. Ce qui l'a remplacé tient à trois refus explicites, chacun
//! avec son message : sans âge vérifié, sans compagnon actif, ou avec un prompt dont l'empreinte
//! ne correspond plus, **le modèle n'est pas appelé**.
//!
//! Les phases suivantes ajoutent, elles ne remplacent plus :
//!
//! ```text
//! phase 1.5 création d'un compagnon depuis Telegram, sans ligne de commande
//! phase 1.6 abonnements et quotas, assis sur le registre de coûts déjà rempli
//! phase 1.8 garde-fous de sortie — les règles fixes tenues par un mécanisme, pas par un prompt
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
//! | [`cli_modele`] | un appel réel au fournisseur, pour l'éprouver et en mesurer le coût |
//! | [`config`] | lecture et validation de l'environnement, au démarrage |
//! | [`error`] | codes d'erreur numériques stables de la surface HTTP |
//! | [`horloge`] | temps, en un seul endroit, pour que les tests puissent le figer |
//! | [`admission`] | ce qu'on retient d'une mise à jour, quelle que soit la porte |
//! | [`db`] | PostgreSQL : connexion, migrations, file à bail, dialogue, registre des coûts |
//! | [`personnage`] | les traits d un compagnon, et le prompt qu ils composent |
//! | [`http`] | routeur, état partagé, sonde de santé |
//! | [`modele`] | le moteur qui écrit les réponses, et son double de test |
//! | [`panne`] | la nature d'un échec de transport, sans l'URL qui l'a causé |
//! | [`secret`] | une valeur qui ne peut pas atterrir dans un journal |
//! | [`scrutation`] | réception sans domaine ni TLS, pour éprouver le bot en vrai |
//! | [`telegram`] | client de l'API Bot : authentification, envoi, découpage |
//! | [`webhook`] | réception des mises à jour |
//! | [`worker`] | file de traitement et production des réponses |

pub mod admission;
pub mod app;
pub mod cli;
pub mod cli_compagnon;
pub mod cli_modele;
pub mod config;
pub mod db;
pub mod error;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
pub mod horloge;
pub mod http;
pub mod modele;
pub mod panne;
pub mod personnage;
pub mod scrutation;
pub mod secret;
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
