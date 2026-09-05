//! Réception des mises à jour envoyées par Telegram.
//!
//! # Ce que ce gestionnaire fait, et surtout ce qu'il ne fait pas
//!
//! Il authentifie, il lit, il met en file, il répond. Il ne parle jamais à Telegram en retour :
//! cela appartient à [`crate::worker`]. Cette séparation est ce qui permettra à la phase 1
//! d'appeler un modèle pendant plusieurs secondes sans que Telegram considère le webhook comme
//! défaillant et rejoue la mise à jour.
//!
//! # Pourquoi un corps illisible reçoit `200`
//!
//! Telegram rejoue toute mise à jour à laquelle on ne répond pas `2xx`. Un corps qu'on ne sait
//! pas lire ne deviendra pas lisible au troisième essai : répondre autre chose que `200`
//! ouvrirait une boucle de rejeu permanente sur un message qu'on ne traitera jamais. On l'absorbe
//! donc, en le journalisant — la trace existe, la boucle non.
//!
//! Le `503` est réservé au cas inverse : la file est pleine, la mise à jour est parfaitement
//! valide, et on *veut* que Telegram la représente plus tard.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;

use tracing::Level;

use crate::error::{ApiError, ErrorCode};
use crate::http::EtatApp;
use crate::telegram::types::{Ecart, Update};

/// Reçoit une mise à jour, déjà authentifiée.
///
/// L'authentification n'est **pas** faite ici mais dans une couche posée devant la route, et
/// ce déplacement n'est pas cosmétique : axum exécute l'extracteur de corps ([`Bytes`], qui
/// draine et collecte la requête) **avant** d'appeler le gestionnaire. Authentifier en
/// première ligne du corps laissait donc n'importe qui, sur une adresse publique, imposer la
/// lecture et l'allocation des 256 Kio de [`crate::http::TAILLE_MAX_CORPS`] sans présenter le
/// moindre secret. La couche, elle, ne voit que les en-têtes et court-circuite avant.
///
/// # Errors
///
/// [`ErrorCode::FileSaturee`] si la file est pleine — Telegram rejouera.
pub async fn recevoir(State(etat): State<EtatApp>, corps: Bytes) -> Result<StatusCode, ApiError> {
    let Some(update) = analyser(&corps) else {
        return Ok(StatusCode::OK);
    };
    let update_id = update.update_id;

    let recu = match update.extraire() {
        Ok(recu) => recu,
        Err(ecart) => {
            journaliser_ecart(update_id, ecart);
            return Ok(StatusCode::OK);
        }
    };

    let chat_id = recu.chat_id;
    let taille = recu.texte.len();

    etat.expediteur.try_send(recu).map_err(|source| {
        // La file pleine n'est pas une défaillance : c'est la contre-pression qui fonctionne.
        // Le `503` demande à Telegram de repasser, ce qu'il fait.
        ApiError::avec_source(
            ErrorCode::FileSaturee,
            "file de traitement saturée, mise à jour non acceptée",
            source,
        )
    })?;

    tracing::info!(update_id, chat_id, octets = taille, "message mis en file");
    Ok(StatusCode::OK)
}

/// Journalise un écart au niveau que l'écart lui-même porte.
///
/// Le niveau vient de [`Ecart::niveau`] et non d'un `match` écrit ici : un bras attrape-tout
/// aurait fait tomber en silence toute variante ajoutée par une phase suivante — `voice` en
/// phase 4 — dans `debug!`, où personne ne l'aurait vue. C'est le même mécanisme que
/// [`ErrorCode::niveau`], et il n'y en a donc qu'un à connaître.
///
/// Aucun code d'erreur HTTP n'est joint : ces mises à jour reçoivent `200`, et accoler un code
/// de réponse à une requête réussie ferait tomber deux situations sans rapport sous le même
/// `grep`.
fn journaliser_ecart(update_id: i64, ecart: Ecart) {
    let motif = ecart.libelle();
    match ecart.niveau() {
        Level::WARN => tracing::warn!(update_id, motif, "mise à jour écartée"),
        Level::INFO => tracing::info!(update_id, motif, "mise à jour écartée"),
        _ => tracing::debug!(update_id, motif, "mise à jour écartée"),
    }
}

/// Lit le corps, ou l'absorbe en le journalisant.
fn analyser(corps: &[u8]) -> Option<Update> {
    match serde_json::from_slice::<Update>(corps) {
        Ok(update) => Some(update),
        Err(erreur) => {
            tracing::warn!(
                code = ErrorCode::PayloadIllisible.code(),
                %erreur,
                octets = corps.len(),
                "corps de webhook inexploitable, absorbé sans rejeu"
            );
            None
        }
    }
}
