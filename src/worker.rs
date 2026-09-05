//! Le consommateur de messages entrants.
//!
//! # Pourquoi le webhook ne répond pas lui-même
//!
//! Telegram attend une réponse HTTP, et rejoue la mise à jour si elle tarde ou échoue.
//! Répondre à l'utilisateur *dans* le gestionnaire de webhook marcherait tant que la réponse
//! est un écho — et cesserait de marcher dès la phase 1, où produire une réponse demande un
//! appel de modèle, puis, phases 3 à 6, une génération d'image ou de vidéo qui se compte en
//! minutes. Le découplage est donc mis en place tout de suite, dans sa forme la plus simple,
//! pour que les phases suivantes remplacent le *contenu* du traitement sans toucher au
//! transport.
//!
//! # Pourquoi une file bornée
//!
//! Une file non bornée transforme un afflux en consommation mémoire jusqu'à l'arrêt brutal du
//! processus. Bornée, elle refuse — et ce refus est exactement le bon signal : le webhook
//! répond `503`, Telegram rejoue la mise à jour plus tard, rien n'est perdu. La contre-pression
//! est déléguée à qui sait déjà la gérer.
//!
//! # Ce que cette file n'est pas encore
//!
//! Elle vit en mémoire. Un arrêt brutal — `kill -9`, panne de courant — perd ce qu'elle
//! contient. L'extinction *ordonnée*, elle, la vide entièrement : voir [`crate::app::Prepare::servir`]. La phase 1
//! remplace ce canal par une file en base à bail, sur le modèle de celle d'`agentbot`, et cette
//! perte-là disparaît. C'est une limite connue et bornée, pas un oubli.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::telegram::Canal;
use crate::telegram::envoi::Action;
use crate::telegram::types::Recu;

/// Nombre de messages en attente de traitement au-delà duquel le service refuse.
///
/// Dimensionné pour absorber une rafale — un rejeu de Telegram après une coupure — sans
/// absorber une inondation.
pub const CAPACITE_FILE: usize = 256;

/// Ce que la phase 0 répond, en attendant qu'un personnage réponde à sa place.
///
/// Le texte dit explicitement ce qu'il est. Un écho muet laisserait croire à un personnage
/// atone ; celui-ci rend visible que le transport marche et que le reste n'existe pas encore.
fn repondre(recu: &Recu) -> String {
    format!(
        "« {} »\n\n(écho — phase 0 : le transport fonctionne, aucun personnage n'est encore branché)",
        recu.texte
    )
}

/// Consomme la file jusqu'à ce qu'elle soit fermée **et** vidée.
///
/// La boucle s'arrête quand tous les émetteurs ont été relâchés et que le dernier message a
/// été traité : c'est ce qui rend l'extinction ordonnée sans perte.
///
/// # Ordonnancement
///
/// Les messages sont traités un par un. C'est volontaire tant que la réponse est immédiate :
/// cela garantit qu'une personne qui écrit deux fois de suite reçoit ses réponses dans
/// l'ordre. Dès que produire une réponse coûtera des secondes, il faudra de la concurrence
/// *entre* conversations en gardant l'ordre *dans* chacune — un changement qui se fera ici,
/// sans toucher au webhook.
pub async fn tourner(mut reception: mpsc::Receiver<Recu>, canal: Arc<Canal>) {
    tracing::info!(capacite = CAPACITE_FILE, "worker démarré");
    let mut traites: u64 = 0;

    while let Some(recu) = reception.recv().await {
        traiter(&canal, &recu).await;
        traites += 1;
    }

    tracing::info!(traites, "worker arrêté, file vidée");
}

/// Traite un message : indication d'activité, puis réponse.
async fn traiter(canal: &Canal, recu: &Recu) {
    // L'indication d'activité est un confort : son échec ne doit pas empêcher la réponse.
    if let Err(erreur) = canal.action(recu.chat_id, Action::Typing).await {
        tracing::debug!(
            chat_id = recu.chat_id,
            %erreur,
            "indication d'activité non affichée"
        );
    }

    let texte = repondre(recu);
    match canal.envoyer_texte(recu.chat_id, &texte).await {
        Ok(identifiants) => tracing::info!(
            chat_id = recu.chat_id,
            message_id = recu.message_id,
            morceaux = identifiants.len(),
            "réponse envoyée"
        ),
        Err(erreur) => {
            // Le niveau distingue ce qui appelle une reprise de ce qui est définitif : un
            // utilisateur qui bloque le bot n'est pas un incident.
            if erreur.merite_une_reprise() {
                tracing::warn!(
                    chat_id = recu.chat_id,
                    attente = ?erreur.attendre(),
                    %erreur,
                    "réponse non envoyée, reprise justifiée"
                );
            } else {
                tracing::info!(
                    chat_id = recu.chat_id,
                    %erreur,
                    "réponse abandonnée, refus définitif de Telegram"
                );
            }
        }
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
        assert!(
            reponse.contains("salut, tu fais quoi ?"),
            "le texte doit être repris"
        );
        assert!(reponse.contains("phase 0"), "l'écho doit dire ce qu'il est");
    }

    #[tokio::test]
    async fn la_file_bornee_refuse_au_dela_de_sa_capacite() {
        let (expediteur, mut reception) = mpsc::channel::<Recu>(CAPACITE_FILE);

        let mut acceptes = 0;
        let mut refuses = 0;
        for numero in 0..CAPACITE_FILE + 10 {
            match expediteur.try_send(recu_de(&format!("message {numero}"))) {
                Ok(()) => acceptes += 1,
                Err(_) => refuses += 1,
            }
        }
        println!("capacité {CAPACITE_FILE} : {acceptes} acceptés, {refuses} refusés");
        assert_eq!(acceptes, CAPACITE_FILE);
        assert_eq!(refuses, 10);

        // Et ce qui a été accepté est bien là, dans l'ordre.
        let premier = reception
            .recv()
            .await
            .expect("la file contient des messages");
        println!("premier message ressorti : {:?}", premier.texte);
        assert_eq!(premier.texte, "message 0");
    }
}
