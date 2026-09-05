//! Réception par scrutation — éprouver le bot sans domaine, sans TLS, sans tunnel.
//!
//! # À quoi cela sert, et à quoi cela ne sert pas
//!
//! Telegram propose deux façons de livrer : il appelle une adresse publique (webhook), ou on
//! vient lui demander (`getUpdates`). La première est celle de la production — elle exige un
//! domaine, un certificat valide et une machine joignable. La seconde ne demande qu'une
//! connexion sortante, donc elle marche depuis n'importe quel poste de travail.
//!
//! Sans ce module, éprouver le bot sur un vrai compte Telegram imposerait un tunnel tiers, un
//! compte de plus et une dépendance externe au premier essai d'un développeur. Avec, il suffit
//! d'un jeton et de `compagnon ecouter`.
//!
//! **Ce n'est pas un raccourci de test.** Ce qui arrive ici passe par la même
//! [`crate::admission::retenir`], la même file et le même worker que le webhook : seule la
//! porte d'entrée change. Un comportement observé en scrutation est donc un comportement réel,
//! et non celui d'un chemin parallèle qui y ressemblerait.
//!
//! La production reste au webhook : la scrutation tient une connexion ouverte en permanence,
//! ne se répartit pas sur plusieurs instances, et redemande à Telegram au lieu d'être servie.
//!
//! # Les deux règles de Telegram qu'il faut respecter
//!
//! 1. **Les deux modes s'excluent.** Tant qu'un webhook est déclaré, `getUpdates` répond `409`.
//!    [`crate::app::scruter`] retire donc le webhook avant d'entrer ici.
//! 2. **Redonner l'`offset` vaut accusé de réception.** Telegram conserve une mise à jour
//!    jusqu'à ce qu'on en réclame une plus récente. Un `offset` qui n'avance pas rejoue le même
//!    lot indéfiniment ; un `offset` avancé trop tôt perd le message.

use std::future::Future;
use std::time::Duration;

use crate::admission::{self, Admission};
use crate::db::Base;
use crate::telegram::Canal;

/// Durée pendant laquelle Telegram garde la connexion ouverte avant de rendre une liste vide.
///
/// Longue à dessein : c'est ce qui distingue une scrutation d'un sondage. À une seconde, on
/// ferait soixante appels par minute pour rien ; à vingt-cinq, une conversation inactive coûte
/// deux appels par minute et un message arrive dans la seconde.
pub const PATIENCE: Duration = Duration::from_secs(25);

/// Repos observé après un appel manqué, avant de réessayer.
///
/// Sans lui, une coupure réseau ou un jeton révoqué produirait une boucle d'appels en échec
/// aussi serrée que la machine le permet.
const REPOS_APRES_ECHEC: Duration = Duration::from_secs(3);

/// Scrute jusqu'à ce que `arret` se réalise.
///
/// Rend la main dès l'arrêt demandé. Ce qu'elle a enfilé et qui n'a pas été traité reste en
/// base : l'appelant arrête ensuite les consommateurs, qui finissent seulement leurs tâches en
/// cours — même contrat d'extinction que le service webhook.
pub async fn tourner(canal: &Canal, base: &Base, arret: impl Future<Output = ()> + Send) {
    // `0` signifie « tout ce qui est en attente ». Le retard accumulé pendant que le bot était
    // éteint est donc livré au démarrage, ce qui est le comportement souhaitable ici : un
    // développeur qui écrit au bot avant de le lancer veut voir ses messages arriver, pas
    // découvrir qu'ils ont été jetés en silence.
    let mut offset: i64 = 0;
    let mut recus: u64 = 0;
    let mut retenus: u64 = 0;

    tracing::info!(
        patience = ?PATIENCE,
        "scrutation démarrée — écrivez au bot depuis Telegram, Ctrl-C pour arrêter"
    );

    tokio::pin!(arret);

    loop {
        let lot = tokio::select! {
            // Biaisé vers l'arrêt : sans cela, `select!` choisit au hasard entre une demande
            // d'arrêt et un appel prêt, et Ctrl-C paraîtrait ignoré une fois sur deux.
            biased;
            () = &mut arret => break,
            resultat = canal.recevoir_mises_a_jour(offset, PATIENCE) => resultat,
        };

        let mises_a_jour = match lot {
            Ok(mises_a_jour) => mises_a_jour,
            Err(erreur) => {
                journaliser_echec(&erreur);
                // Le repos est lui aussi interruptible : sinon Ctrl-C attendrait jusqu'à trois
                // secondes pour être pris en compte.
                tokio::select! {
                    biased;
                    () = &mut arret => break,
                    () = tokio::time::sleep(REPOS_APRES_ECHEC) => continue,
                }
            }
        };

        if mises_a_jour.is_empty() {
            continue;
        }
        recus += mises_a_jour.len() as u64;
        tracing::debug!(lot = mises_a_jour.len(), "lot reçu");

        for update in mises_a_jour {
            let suivant = update.update_id + 1;

            if let Some(recu) = admission::retenir(update) {
                tracing::info!(
                    chat_id = recu.chat_id,
                    prenom = %recu.prenom,
                    octets = recu.texte.len(),
                    "message reçu"
                );
                match admission::enfiler(base, &recu).await {
                    Ok(Admission::Enfile) => retenus += 1,
                    // La borne est atteinte : contrairement au webhook, personne ne rejouera
                    // pour nous. On avance quand même l'`offset` — sinon Telegram redonnerait
                    // ce message sans fin, et la scrutation tournerait à vide sur lui.
                    Ok(Admission::Sature) => tracing::warn!(
                        chat_id = recu.chat_id,
                        "borne de file atteinte, message abandonné"
                    ),
                    Err(erreur) => {
                        tracing::error!(%erreur, "mise en file impossible, scrutation interrompue");
                        return;
                    }
                }
            }

            // L'`offset` n'avance qu'une fois la mise à jour prise en charge : avancé avant,
            // il l'accuserait auprès de Telegram sans qu'elle soit traitée nulle part.
            offset = offset.max(suivant);
        }
    }

    tracing::info!(recus, retenus, "scrutation arrêtée");
}

/// Journalise un appel manqué, en nommant le cas que l'exploitant peut réparer.
fn journaliser_echec(erreur: &crate::telegram::envoi::ErreurEnvoi) {
    use crate::telegram::envoi::ErreurEnvoi;

    // `409` mérite son propre message : c'est le seul échec dont la cause est une erreur de
    // conduite plutôt qu'une panne, et son intitulé brut n'aide personne.
    if let ErreurEnvoi::Api { code: 409, .. } = erreur {
        tracing::error!(
            "Telegram refuse la scrutation : un webhook est encore déclaré, ou une autre \
             instance écoute déjà avec le même jeton. Retirer le webhook avec \
             « compagnon webhook retirer », et n'exécuter qu'un seul « compagnon ecouter »."
        );
        return;
    }
    tracing::warn!(%erreur, "appel de scrutation manqué, nouvelle tentative");
}
