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

use tracing::Level;

use crate::db::{Base, ErreurBase, file, utilisateurs};
use crate::telegram::types::{Ecart, Recu, Update};

/// Le type de tâche produit par un message entrant.
const TACHE_MESSAGE: &str = "message_entrant";

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

/// Ce qu'il est advenu d'un message retenu.
#[derive(Debug)]
pub enum Admission {
    /// Enfilé, un worker le prendra.
    Enfile,
    /// Refusé : cet utilisateur a déjà trop de tâches en attente.
    Sature,
}

/// Inscrit l'utilisateur s'il est inconnu, puis enfile le message.
///
/// Les deux gestes vont ensemble et dans cet ordre : `file_messages.utilisateur_id` porte une
/// clé étrangère, donc enfiler avant d'inscrire échouerait. Les regrouper ici évite que les
/// deux portes d'entrée — webhook et scrutation — n'en donnent deux versions qui divergeraient.
///
/// Le prénom est mis à jour au passage : c'est la seule occasion de le faire, Telegram ne le
/// livrant qu'avec un message.
///
/// # Errors
///
/// [`ErreurBase`] si la base refuse l'une des deux écritures.
pub async fn enfiler(base: &Base, recu: &Recu) -> Result<Admission, ErreurBase> {
    // La résolution `(canal, identifiant externe) → utilisateur` est le premier traitement de
    // toute requête entrante. Au-delà de cette ligne, plus rien ne connaît Telegram : la file,
    // le worker et le moteur de dialogue ne manipulent que l'UUID interne.
    let utilisateur = utilisateurs::resoudre_telegram(
        base.pool(),
        recu.utilisateur_telegram,
        Some(&recu.prenom),
    )
    .await?;

    let enfilee = file::enfiler(base.pool(), utilisateur, TACHE_MESSAGE, recu).await?;
    Ok(if enfilee.is_some() {
        Admission::Enfile
    } else {
        Admission::Sature
    })
}
