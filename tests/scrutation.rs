//! La scrutation emprunte le même chemin que le webhook, et accuse ce qu'elle a pris.
//!
//! # Ce que ces tests protègent
//!
//! Un mode d'écoute de développement n'a de valeur que s'il traverse le code de production.
//! S'il empruntait un chemin parallèle, éprouver le bot en scrutation ne dirait rien de son
//! comportement une fois déployé — et le confort du développeur aurait été payé par une
//! garantie fausse. Ces tests vérifient donc l'identité des deux chemins, pas leur existence.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use std::time::Duration;

use std::sync::Arc;

use compagnon::app;
use compagnon::modele::double::ModeleDouble;
use harnais::{FauxTelegram, update_privee};
use tokio::sync::oneshot;

/// Démarre la scrutation contre le faux Telegram, et rend de quoi l'arrêter.
async fn lancer(
    faux: &FauxTelegram,
    base: &harnais::base::BaseDeTest,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let config = faux.config(&base.url);
    // La scrutation partage le worker du service webhook : elle a donc besoin du même modèle,
    // et c'est précisément ce que le test veut prouver — les deux portes mènent au même code.
    let modele = Arc::new(ModeleDouble::qui_repete());
    let (arret, reception) = oneshot::channel();
    let tache = tokio::spawn(async move {
        app::scruter(&config, modele, Arc::new(compagnon::fixtures::sceau_de_test()), async move {
            let _ = reception.await;
        })
        .await
        .expect("la scrutation ne doit pas s'interrompre sur une erreur");
    });
    (arret, tache)
}

#[tokio::test]
async fn un_message_scrute_ressort_par_la_meme_porte_qu_un_message_webhook() {
    let faux = FauxTelegram::demarrer().await;
    faux.livrer(vec![update_privee(970_001, "salut par scrutation")])
        .await;

    let base = harnais::base::BaseDeTest::creer().await;
    base.prete_a_converser(harnais::UTILISATEUR, "Alix").await;
    let (arret, tache) = lancer(&faux, &base).await;

    // La réponse doit partir exactement comme si le message était entré par le webhook.
    let messages = faux.attendre("sendMessage", 1).await;
    println!(
        "ce que le service a envoyé à Telegram :\n{}",
        faux.journal().await
    );

    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("\ntexte envoyé :\n---\n{texte}\n---");
    assert_eq!(messages[0]["chat_id"], 42);
    assert_eq!(
        texte, "salut par scrutation",
        "le modèle a reçu le message et sa réponse est partie par la même porte"
    );

    // L'indication d'activité aussi : c'est le worker de production qui tourne, pas un
    // raccourci propre à la scrutation.
    let actions = faux.attendre("sendChatAction", 1).await;
    assert_eq!(actions[0]["action"], "typing");
    println!("action affichée : {}", actions[0]["action"]);

    let _ = arret.send(());
    tache.await.expect("arrêt propre");
    base.detruire().await;
}

#[tokio::test]
async fn le_webhook_est_retire_avant_toute_scrutation() {
    let faux = FauxTelegram::demarrer().await;
    faux.livrer(vec![]).await;

    let base = harnais::base::BaseDeTest::creer().await;
    let (arret, tache) = lancer(&faux, &base).await;

    // Telegram interdit de mêler les deux modes : sans ce retrait, chaque `getUpdates`
    // répondrait 409 et la scrutation ne recevrait jamais rien.
    let retraits = faux.attendre("deleteWebhook", 1).await;
    println!("deleteWebhook appelé {} fois", retraits.len());

    // Attendre la première scrutation AVANT de lire l'ordre. Sans cette attente le test est
    // une course : entre `deleteWebhook` et le premier `getUpdates`, le service joint la base
    // et applique ses migrations. Le test passait jusqu'ici parce que l'intervalle était de
    // quelques microsecondes — il ne prouvait rien, il gagnait.
    faux.attendre("getUpdates", 1).await;
    let ordre = faux.ordre_des_appels().await;
    println!("ordre des appels : {:?}", &ordre[..ordre.len().min(4)]);

    let retrait = ordre
        .iter()
        .position(|m| m == "deleteWebhook")
        .expect("deleteWebhook doit avoir été appelé");
    let premiere_scrutation = ordre
        .iter()
        .position(|m| m == "getUpdates")
        .expect("getUpdates doit avoir été appelé");
    println!("deleteWebhook en position {retrait}, premier getUpdates en {premiere_scrutation}");
    assert!(
        retrait < premiere_scrutation,
        "le webhook doit être retiré AVANT le premier getUpdates, sinon Telegram répond 409"
    );

    let _ = arret.send(());
    tache.await.expect("arrêt propre");
    base.detruire().await;
}

#[tokio::test]
async fn l_offset_avance_pour_accuser_ce_qui_a_ete_pris() {
    let faux = FauxTelegram::demarrer().await;
    // Trois mises à jour d'un coup, aux identifiants non contigus — Telegram ne promet pas
    // qu'ils le soient.
    faux.livrer(vec![
        update_privee(980_010, "un"),
        update_privee(980_011, "deux"),
        update_privee(980_030, "trois"),
    ])
    .await;

    let base = harnais::base::BaseDeTest::creer().await;
    base.prete_a_converser(harnais::UTILISATEUR, "Alix").await;
    let (arret, tache) = lancer(&faux, &base).await;

    // Les trois doivent ressortir, dans l'ordre.
    let messages = faux.attendre("sendMessage", 3).await;
    let textes: Vec<&str> = messages
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default())
        .collect();
    for (rang, texte) in textes.iter().enumerate() {
        println!("{rang} : {}", texte.lines().next().unwrap_or_default());
    }
    assert_eq!(textes, ["un", "deux", "trois"]);

    // Redonner l'offset est ce qui ACQUITTE auprès de Telegram : sans progression, le même lot
    // serait rejoué indéfiniment. Le premier appel part de 0, les suivants doivent dépasser le
    // plus grand identifiant vu.
    let offsets: Vec<i64> = faux
        .corps("getUpdates")
        .await
        .iter()
        .map(|c| c["offset"].as_i64().unwrap_or(-1))
        .collect();
    println!(
        "\noffsets demandés : {:?}",
        &offsets[..offsets.len().min(6)]
    );
    assert_eq!(offsets[0], 0, "le premier appel réclame tout ce qui attend");
    assert!(
        offsets.contains(&980_031),
        "l'offset doit dépasser le plus grand identifiant reçu (980030), sinon le lot est rejoué"
    );

    let _ = arret.send(());
    tache.await.expect("arrêt propre");
    base.detruire().await;
}

#[tokio::test]
async fn une_mise_a_jour_ecartee_avance_quand_meme_l_offset() {
    let faux = FauxTelegram::demarrer().await;
    // Un message de groupe : écarté par l'admission, mais il doit être ACQUITTÉ malgré tout.
    // Sans progression de l'offset, Telegram le redonnerait sans fin et bloquerait la file
    // derrière lui — le bot paraîtrait figé sans qu'aucune erreur ne soit journalisée.
    let groupe = serde_json::json!({
        "update_id": 990_005,
        "message": {
            "message_id": 5,
            "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
            "chat": {"id": -1_001_234, "type": "supergroup"},
            "date": 1_760_000_000_i64,
            "text": "coucou le groupe"
        }
    });
    faux.livrer(vec![groupe]).await;

    let base = harnais::base::BaseDeTest::creer().await;
    let (arret, tache) = lancer(&faux, &base).await;

    // Attendre que l'offset ait dépassé la mise à jour écartée.
    let limite = std::time::Instant::now() + Duration::from_secs(5);
    let mut vu = false;
    while std::time::Instant::now() < limite {
        let offsets: Vec<i64> = faux
            .corps("getUpdates")
            .await
            .iter()
            .map(|c| c["offset"].as_i64().unwrap_or(-1))
            .collect();
        if offsets.contains(&990_006) {
            println!("offsets demandés : {offsets:?}");
            vu = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        vu,
        "une mise à jour écartée doit être acquittée, pas rejouée sans fin"
    );

    let messages = faux.corps("sendMessage").await;
    println!("réponses envoyées : {} (attendu : 0)", messages.len());
    assert!(
        messages.is_empty(),
        "un message de groupe ne doit produire aucune réponse"
    );

    let _ = arret.send(());
    tache.await.expect("arrêt propre");
    base.detruire().await;
}
