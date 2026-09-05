//! Ce qu'on retient d'une mise à jour, quelle que soit la façon dont elle est arrivée.
//!
//! # Pourquoi ce module existe
//!
//! Une mise à jour peut entrer par deux portes : le webhook, en production, et la scrutation,
//! sur un poste de travail. Ce qui se passe **ensuite** doit être rigoureusement identique,
//! sinon éprouver le bot en scrutation ne dirait rien de son comportement en production — et
//! le harnais perdrait sa raison d'être.
//!
//! Le filtrage et sa journalisation vivent donc ici, à un seul endroit, et les deux portes
//! l'appellent. C'est aussi l'endroit nommé où la politique d'admission grossira : un
//! utilisateur banni, un quota, un personnage qui accepterait les groupes.

use crate::telegram::types::{Ecart, Recu, Update};
use tracing::Level;

/// Retient ce qui mérite une réponse, en journalisant l'écart le cas échéant.
///
/// Renvoie `None` quand la mise à jour n'appelle aucune réponse — un autocollant, un message
/// de groupe, une correction. Ce n'est pas une erreur : c'est le fonctionnement normal, et
/// l'appelant doit accuser réception malgré tout.
#[must_use]
pub fn retenir(update: Update) -> Option<Recu> {
    let update_id = update.update_id;
    match update.extraire() {
        Ok(recu) => Some(recu),
        Err(ecart) => {
            journaliser(update_id, ecart);
            None
        }
    }
}

/// Journalise un écart au niveau que l'écart lui-même porte.
///
/// Le niveau vient de [`Ecart::niveau`] et non d'un `match` écrit ici : un bras attrape-tout
/// ferait tomber en silence toute variante ajoutée par une phase suivante — `voice` en phase 4
/// — dans `debug!`, où personne ne la verrait.
///
/// Aucun code d'erreur HTTP n'est joint : ces mises à jour sont acquittées, et accoler un code
/// de réponse à une requête réussie ferait tomber deux situations sans rapport sous le même
/// `grep`.
fn journaliser(update_id: i64, ecart: Ecart) {
    let motif = ecart.libelle();
    match ecart.niveau() {
        Level::WARN => tracing::warn!(update_id, motif, "mise à jour écartée"),
        Level::INFO => tracing::info!(update_id, motif, "mise à jour écartée"),
        _ => tracing::debug!(update_id, motif, "mise à jour écartée"),
    }
}
