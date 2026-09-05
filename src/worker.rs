//! Les consommateurs de la file, et ce qu'ils font d'une tâche.
//!
//! # Pourquoi le webhook ne répond pas lui-même
//!
//! Telegram attend une réponse HTTP et rejoue la mise à jour si elle tarde. Répondre dans le
//! gestionnaire marcherait tant que la réponse est un écho, et cesserait de marcher dès qu'elle
//! coûte un appel de modèle — puis une génération d'image qui se compte en minutes.
//!
//! # Concurrence : entre les conversations, pas dans une conversation
//!
//! Plusieurs workers tournent en parallèle. Ce n'était pas le cas en phase 0, où le traitement
//! sérialisé garantissait gratuitement l'ordre des réponses — et où l'écho coûtait cinquante
//! millisecondes. Dès qu'une réponse coûte des secondes, ce même sérialisme fait attendre la
//! centième personne pendant cinq minutes, sans qu'aucune erreur ne soit journalisée : le bot
//! paraît simplement mort.
//!
//! L'ordre reste tenu là où il compte — dans une conversation — par la requête de prise, qui
//! écarte tout utilisateur déjà servi ailleurs (voir [`crate::db::file`]). Le worker n'a donc
//! aucune synchronisation à faire : la base la lui donne.
//!
//! # Pourquoi une scrutation et pas une notification
//!
//! Les workers interrogent la file à intervalle court plutôt que d'être réveillés par un
//! `LISTEN/NOTIFY`. C'est un compromis assumé pour cette phase : la latence ajoutée est bornée
//! par `REPOS_MAX` (250 ms), et l'absence de canal de notification retire une pièce mobile au moment
//! où la file elle-même est neuve. À reprendre quand la latence comptera davantage que la
//! simplicité — c'est-à-dire quand la réponse ne sera plus un écho.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::db::{Base, file, utilisateurs};
use crate::error::ErrorCode;
use crate::telegram::Canal;
use crate::telegram::envoi::Action;
use crate::telegram::types::Recu;

/// Nombre de consommateurs lancés en parallèle.
///
/// Quatre plutôt qu'un par cœur : le travail est presque entièrement de l'attente réseau, pas
/// du calcul. La borne réelle est ailleurs — le débit que Telegram accepte, et le nombre de
/// connexions du pool.
pub const WORKERS: usize = 4;

/// Durée du bail posé sur une tâche prise.
///
/// Généreuse par rapport au coût d'un écho, parce qu'elle doit couvrir le cas le plus lent, pas
/// le plus fréquent : un bail trop court ferait reprendre par un second worker une tâche que le
/// premier est encore en train de traiter, et l'utilisateur recevrait deux réponses.
const BAIL: Duration = Duration::from_secs(120);

/// Nombre de prises au-delà duquel une tâche est abandonnée.
const TENTATIVES_MAX: i16 = 3;

/// Repos après une tâche traitée : court, car il y en a probablement une autre.
const REPOS_MIN: Duration = Duration::from_millis(25);

/// Repos quand la file est vide. Borne haute de la latence ajoutée par la scrutation.
const REPOS_MAX: Duration = Duration::from_millis(250);

/// Ce que la phase 1.1 répond, en attendant qu'un compagnon réponde à sa place.
fn repondre(recu: &Recu) -> String {
    format!(
        "« {} »\n\n(écho — phase 1.1 : la file est en base et survit à un arrêt brutal, aucun compagnon n'est encore branché)",
        recu.texte
    )
}

/// Ce que reçoit quelqu'un dont l'âge n'est pas vérifié.
///
/// Un refus muet serait indiscernable d'une panne — c'est la première friction que la carte des
/// parcours signale. Le message dit ce qui manque, sans jouer de personnage : la vérification
/// d'âge est une limite de service, et la présenter autrement serait malhonnête.
const VERIFICATION_REQUISE: &str = "Avant de commencer, ce service demande une vérification d'âge.\n\n\
     Elle n'est pas encore disponible — cette phase met en place la persistance. \
     Reviens quand l'inscription sera ouverte.";

/// Consomme la file jusqu'à ce que l'arrêt soit demandé.
///
/// Termine la tâche en cours avant de rendre la main : une tâche interrompue serait reprise par
/// le bail, mais l'utilisateur recevrait sa réponse deux fois.
pub async fn tourner(
    base: Base,
    canal: Arc<Canal>,
    mut arret: watch::Receiver<bool>,
    numero: usize,
) {
    tracing::debug!(numero, "worker démarré");
    let mut traites: u64 = 0;

    loop {
        if *arret.borrow() {
            break;
        }

        let tache = match file::prendre(base.pool(), BAIL).await {
            Ok(Some(tache)) => tache,
            Ok(None) => {
                // Rien à prendre : soit la file est vide, soit tout ce qu'elle contient
                // appartient à des utilisateurs déjà servis ailleurs.
                tokio::select! {
                    biased;
                    _ = arret.changed() => break,
                    () = tokio::time::sleep(REPOS_MAX) => continue,
                }
            }
            Err(erreur) => {
                tracing::error!(numero, %erreur, "file inaccessible");
                tokio::select! {
                    biased;
                    _ = arret.changed() => break,
                    () = tokio::time::sleep(REPOS_MAX) => continue,
                }
            }
        };

        traiter(&base, &canal, &tache).await;
        traites += 1;

        tokio::select! {
            biased;
            _ = arret.changed() => break,
            () = tokio::time::sleep(REPOS_MIN) => {}
        }
    }

    tracing::debug!(numero, traites, "worker arrêté");
}

/// Traite une tâche prise, et la rend à la file dans tous les cas.
async fn traiter(base: &Base, canal: &Canal, tache: &file::Tache) {
    let Ok(recu) = serde_json::from_value::<Recu>(tache.charge_utile.clone()) else {
        tracing::error!(
            tache = %tache.id,
            code = ErrorCode::TacheIllisible.code(),
            "charge utile illisible, tâche abandonnée"
        );
        rendre_en_echec(base, tache, ErrorCode::TacheIllisible).await;
        return;
    };

    // La vérification d'âge est demandée ici et non à l'entrée : c'est le worker qui parle à
    // l'utilisateur, et un refus doit produire une réponse plutôt qu'un silence. En phase 1.3,
    // c'est au même endroit qu'elle empêchera l'appel au modèle.
    let verifie = match utilisateurs::age_verifie(base.pool(), recu.utilisateur_id).await {
        Ok(verifie) => verifie,
        Err(erreur) => {
            tracing::error!(tache = %tache.id, %erreur, "vérification d'âge impossible");
            rendre_en_echec(base, tache, ErrorCode::Interne).await;
            return;
        }
    };

    let texte = if verifie {
        repondre(&recu)
    } else {
        tracing::info!(
            chat_id = recu.chat_id,
            "âge non vérifié, accès au moteur refusé"
        );
        VERIFICATION_REQUISE.to_owned()
    };

    // L'indication d'activité est un confort : son échec ne doit pas empêcher la réponse.
    if let Err(erreur) = canal.action(recu.chat_id, Action::Typing).await {
        tracing::debug!(chat_id = recu.chat_id, %erreur, "indication d'activité non affichée");
    }

    match canal.envoyer_texte(recu.chat_id, &texte).await {
        Ok(identifiants) => {
            tracing::info!(
                chat_id = recu.chat_id,
                message_id = recu.message_id,
                morceaux = identifiants.len(),
                "réponse envoyée"
            );
            if let Err(erreur) = file::terminer(base.pool(), tache.id).await {
                // La réponse est partie ; la tâche sera reprise au bail et renverra la même
                // chose. C'est le seul point où un doublon reste possible, et il vaut mieux
                // qu'une réponse perdue.
                tracing::error!(tache = %tache.id, %erreur, "tâche traitée mais non close");
            }
        }
        Err(erreur) => {
            if erreur.merite_une_reprise() {
                tracing::warn!(
                    chat_id = recu.chat_id,
                    tentative = tache.tentatives,
                    attente = ?erreur.attendre(),
                    %erreur,
                    "réponse non envoyée, tâche remise en file"
                );
                rendre_en_echec(base, tache, ErrorCode::EnvoiImpossible).await;
            } else {
                // Un utilisateur qui bloque le bot n'est pas un incident, et réessayer
                // referait exactement la même erreur : la tâche est close, pas reprise.
                tracing::info!(chat_id = recu.chat_id, %erreur, "refus définitif de Telegram");
                if let Err(erreur) = file::terminer(base.pool(), tache.id).await {
                    tracing::error!(tache = %tache.id, %erreur, "tâche abandonnée mais non close");
                }
            }
        }
    }
}

/// Rend une tâche en échec, en journalisant si même cela échoue.
async fn rendre_en_echec(base: &Base, tache: &file::Tache, code: ErrorCode) {
    if let Err(erreur) = file::echouer(
        base.pool(),
        tache.id,
        i32::from(code.code()),
        TENTATIVES_MAX,
    )
    .await
    {
        tracing::error!(tache = %tache.id, %erreur, "tâche en échec et non rendue");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn recu_de(texte: &str) -> Recu {
        Recu {
            chat_id: 42,
            utilisateur_id: 42,
            message_id: 17,
            prenom: "Erwan".to_owned(),
            texte: texte.to_owned(),
            recu_le: 1_760_000_000,
        }
    }

    #[test]
    fn l_echo_reprend_le_texte_et_annonce_ce_qu_il_est() {
        let reponse = repondre(&recu_de("salut, tu fais quoi ?"));
        println!("réponse produite :\n---\n{reponse}\n---");
        assert!(reponse.contains("salut, tu fais quoi ?"));
        assert!(reponse.contains("phase 1.1"));
    }

    #[test]
    fn un_recu_survit_a_un_aller_retour_par_la_base() {
        // La charge utile transite en `jsonb` : ce qui ressort doit être ce qui est entré,
        // sinon une tâche reprise après un redémarrage répondrait à côté.
        let avant = recu_de("un aller-retour en jsonb, avec des accents et un emoji 🙂");
        let json = serde_json::to_value(&avant).expect("Recu sérialisable");
        println!("charge utile : {json}");
        let apres: Recu = serde_json::from_value(json).expect("Recu relisible");
        println!("texte relu   : {}", apres.texte);
        assert_eq!(apres.texte, avant.texte);
        assert_eq!(apres.chat_id, avant.chat_id);
        assert_eq!(apres.utilisateur_id, avant.utilisateur_id);
        assert_eq!(apres.message_id, avant.message_id);
    }

    #[test]
    fn le_message_de_verification_ne_joue_pas_de_personnage() {
        println!("---\n{VERIFICATION_REQUISE}\n---");
        assert!(VERIFICATION_REQUISE.contains("vérification d'âge"));
    }
}
