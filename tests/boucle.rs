//! La boucle de bout en bout : Telegram appelle, le service répond à Telegram.
//!
//! Chaque test part d'un service réellement démarré sur une socket, et n'observe que ce que le
//! faux Telegram a **reçu** — jamais l'état interne du service. C'est ce qui rend ces tests
//! indépendants de l'implémentation : la phase 1 remplacera l'écho par un modèle, et ces tests
//! continueront de dire la vérité sur le transport.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use harnais::{FauxTelegram, update_privee};

#[tokio::test]
async fn un_message_prive_traverse_tout_le_circuit_et_revient_en_echo() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;
    println!("service à l'écoute sur {}", service.adresse);

    let reponse = service
        .poster(&update_privee(900_001, "salut, tu fais quoi ?"))
        .await;
    println!("webhook -> {} ", reponse.status());
    assert_eq!(
        reponse.status(),
        200,
        "Telegram ne doit pas avoir à rejouer"
    );

    let messages = faux.attendre("sendMessage", 1).await;
    println!(
        "\nce que le service a envoyé à Telegram :\n{}",
        faux.journal().await
    );

    let envoye = &messages[0];
    assert_eq!(
        envoye["chat_id"], 42,
        "la réponse doit partir dans la bonne discussion"
    );
    let texte = envoye["text"].as_str().unwrap_or_default();
    println!("\ntexte envoyé :\n---\n{texte}\n---");
    assert!(
        texte.contains("salut, tu fais quoi ?"),
        "l'écho doit reprendre le message reçu"
    );

    // L'indication d'activité part avant la réponse : c'est ce qui donne l'illusion.
    let actions = faux.attendre("sendChatAction", 1).await;
    println!("action affichée : {}", actions[0]["action"]);
    assert_eq!(actions[0]["action"], "typing");

    service.eteindre().await;
}

#[tokio::test]
async fn un_secret_errone_est_refuse_sans_qu_un_seul_octet_parte_chez_telegram() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    let cas = [
        (
            "secret d'un autre déploiement",
            "un-autre-secret-de-quarante-huit-caracteres-abcd",
        ),
        ("secret vide", ""),
        (
            "secret tronqué d'un caractère",
            &harnais::SECRET[..harnais::SECRET.len() - 1],
        ),
    ];

    for (nom, secret) in cas {
        let reponse = service
            .poster_avec_secret(&update_privee(900_002, "laisse-moi entrer"), secret)
            .await;
        let statut = reponse.status();
        let corps: serde_json::Value = reponse.json().await.expect("le refus est du JSON");
        println!("{nom:32} -> {statut} {corps}");

        assert_eq!(statut, 401);
        assert_eq!(
            corps["code"], 1001,
            "le code doit être stable pour les clients"
        );
        assert_eq!(
            corps["message"], "requête non authentifiée",
            "le message ne doit pas dire *laquelle* des vérifications a échoué"
        );
    }

    // Le point important : rien n'est parti. Un service qui répondrait quand même serait un
    // relais ouvert vers les utilisateurs du bot.
    service.eteindre().await;
    let messages = faux.corps("sendMessage").await;
    println!(
        "\nappels à sendMessage après trois refus : {}",
        messages.len()
    );
    println!("journal complet :\n{}", faux.journal().await);
    assert!(
        messages.is_empty(),
        "aucune réponse ne doit partir sur un appel non authentifié"
    );
}

#[tokio::test]
async fn un_corps_illisible_est_absorbe_sans_ouvrir_de_boucle_de_rejeu() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Telegram rejoue tout ce qui n'est pas 2xx. Un corps qu'on ne saura jamais lire doit
    // donc être absorbé, pas refusé.
    let reponse = service.poster_brut("{ceci n'est pas du JSON").await;
    println!("corps illisible      -> {}", reponse.status());
    assert_eq!(
        reponse.status(),
        200,
        "un rejeu ne rendrait pas ce corps lisible"
    );

    // Une mise à jour valide mais sans message exploitable : même traitement.
    let sans_message = serde_json::json!({"update_id": 900_003});
    let reponse = service.poster(&sans_message).await;
    println!("mise à jour sans message -> {}", reponse.status());
    assert_eq!(reponse.status(), 200);

    // Un message de groupe : écarté, mais accusé réception.
    let groupe = serde_json::json!({
        "update_id": 900_004,
        "message": {
            "message_id": 5,
            "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
            "chat": {"id": -1_001_234, "type": "supergroup"},
            "date": 1_760_000_000_i64,
            "text": "@compagnon_de_test_bot viens ici"
        }
    });
    let reponse = service.poster(&groupe).await;
    println!("message de groupe    -> {}", reponse.status());
    assert_eq!(reponse.status(), 200);

    service.eteindre().await;
    let messages = faux.corps("sendMessage").await;
    println!("\nappels à sendMessage : {}", messages.len());
    println!("journal complet :\n{}", faux.journal().await);
    assert!(
        messages.is_empty(),
        "aucun de ces trois cas ne doit produire de réponse"
    );
}

#[tokio::test]
async fn une_reponse_trop_longue_part_en_plusieurs_messages() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // L'entrant est plafonné à 4096 unités UTF-16 — au-delà, il est écarté avant la file.
    // C'est donc l'**enrobage** de la réponse qui fait franchir la limite : un message
    // entrant juste sous le plafond produit une réponse juste au-dessus. Le cas est étroit et
    // c'est exactement pour cela qu'il mérite un test — c'est celui qu'on n'atteint jamais à
    // la main, et celui où Telegram rejette tout le message sans rien afficher.
    // Construit à la taille maximale que Telegram accepte en entrée : de cette façon, le
    // test tient quel que soit l'enrobage que les phases suivantes mettront autour de la
    // réponse — dès qu'il est non vide, la limite sortante est franchie.
    let motif = "Elle repose sa tasse et te regarde sans rien dire. ";
    let long: String = motif.chars().cycle().take(4096).collect();
    println!(
        "message entrant : {} unités UTF-16 (plafond entrant : 4096)",
        harnais::longueur_utf16(&long)
    );

    let reponse = service.poster(&update_privee(900_005, &long)).await;
    assert_eq!(reponse.status(), 200);

    let messages = faux.attendre("sendMessage", 2).await;
    println!("réponse découpée en {} messages :", messages.len());
    for (rang, message) in messages.iter().enumerate() {
        let texte = message["text"].as_str().unwrap_or_default();
        let unites = harnais::longueur_utf16(texte);
        println!("  {rang} : {unites} unités UTF-16");
        assert!(
            unites <= 4096,
            "le morceau {rang} dépasse la limite de Telegram"
        );
    }
    assert!(messages.len() >= 2, "cette réponse doit être découpée");

    service.eteindre().await;
}

#[tokio::test]
async fn l_extinction_ordonnee_traite_tout_ce_qui_avait_ete_accepte() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Le point délicat : le service accepte, répond 200, puis on l'éteint immédiatement. Rien
    // ne doit disparaître — c'est la garantie que la file en mémoire ne perd pas ce qu'elle a
    // accusé, et le contrat que la file durable de la phase 1 devra tenir à son tour.
    const COMBIEN: i64 = 12;
    for numero in 0..COMBIEN {
        let reponse = service
            .poster(&update_privee(
                910_000 + numero,
                &format!("message {numero}"),
            ))
            .await;
        assert_eq!(reponse.status(), 200, "message {numero} refusé");
    }
    println!("{COMBIEN} messages acceptés, extinction immédiate demandée");

    service.eteindre().await;

    let messages = faux.corps("sendMessage").await;
    println!(
        "réponses parties avant l'arrêt complet : {}",
        messages.len()
    );
    for (rang, message) in messages.iter().enumerate() {
        let texte = message["text"].as_str().unwrap_or_default();
        let premiere_ligne = texte.lines().next().unwrap_or_default();
        println!("  {rang:>2} : {premiere_ligne}");
    }
    assert_eq!(
        messages.len(),
        COMBIEN as usize,
        "l'extinction a perdu {} message(s)",
        COMBIEN as usize - messages.len()
    );

    // Et dans l'ordre où ils sont arrivés.
    for (rang, message) in messages.iter().enumerate() {
        let texte = message["text"].as_str().unwrap_or_default();
        assert!(
            texte.contains(&format!("message {rang}")),
            "le message {rang} n'est pas à sa place"
        );
    }
    println!("ordre respecté sur les {COMBIEN} messages");
}

#[tokio::test]
async fn le_contrat_d_erreur_vaut_aussi_pour_les_reponses_du_routeur() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Ces réponses ne viennent pas d'un gestionnaire : elles sortiraient nues sans la couche
    // d'enveloppe. Un client qui lit `{"code":...}` partout ailleurs tomberait sur du vide.
    let reponse = service.obtenir("/route-qui-n-existe-pas").await;
    let statut = reponse.status();
    let corps: serde_json::Value = reponse.json().await.expect("même une 404 est du JSON");
    println!("GET /route-qui-n-existe-pas -> {statut} {corps}");
    assert_eq!(statut, 404);
    assert_eq!(corps["code"], 2004);

    // `POST /health` : la route existe, la méthode non, et aucune authentification ne la
    // protège — c'est donc là que le 405 reste observable.
    let reponse = service.poster_sur("/health").await;
    let statut = reponse.status();
    let corps: serde_json::Value = reponse.json().await.expect("même une 405 est du JSON");
    println!("POST /health                -> {statut} {corps}");
    assert_eq!(statut, 405);
    assert_eq!(corps["code"], 2005);

    service.eteindre().await;
}

#[tokio::test]
async fn une_methode_non_autorisee_sur_le_webhook_repond_d_abord_non_authentifiee() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Choix délibéré, et conséquence du déplacement de l'authentification en couche : elle
    // s'exécute avant que le routeur ne constate que la méthode ne convient pas. Un appelant
    // sans secret n'apprend donc pas quelles méthodes /webhook accepte — ce qui prolonge la
    // règle déjà tenue sur les trois modes d'échec du secret : ne rien dire de plus que
    // « non authentifié ».
    let reponse = service.obtenir("/webhook").await;
    let statut = reponse.status();
    let corps: serde_json::Value = reponse.json().await.expect("le refus est du JSON");
    println!("GET /webhook (sans secret)  -> {statut} {corps}");
    assert_eq!(
        statut, 401,
        "l'authentification doit précéder le routage de méthode"
    );
    assert_eq!(corps["code"], 1001);

    service.eteindre().await;
}

#[tokio::test]
async fn la_sonde_dit_la_version_et_l_etat_de_la_file() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    let sante = service.sante().await;
    println!("GET /health -> {sante}");
    assert_eq!(sante["statut"], "ok");
    assert_eq!(sante["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        sante["file_libre"], sante["file_capacite"],
        "au repos, la file doit être entièrement libre"
    );

    service.eteindre().await;
}

#[tokio::test]
async fn un_appel_non_authentifie_est_refuse_avant_que_son_corps_ne_soit_lu() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Ce qui est vérifié n'est pas le statut mais l'ORDRE des opérations.
    //
    // Axum exécute les extracteurs — dont `Bytes`, qui draine et collecte la requête — puis
    // seulement appelle le gestionnaire. Tant que l'authentification était la première ligne
    // du gestionnaire, elle arrivait donc APRÈS la lecture du corps : n'importe qui, sur une
    // adresse publique, imposait la lecture et l'allocation de TAILLE_MAX_CORPS sans présenter
    // le moindre secret.
    //
    // Une requête ordinaire ne distingue pas les deux ordres — le refus est identique. On
    // annonce donc un corps qu'on n'envoie jamais :
    //   - corps lu en premier   -> le service attend, et ne répond qu'à l'expiration du délai
    //                              de requête (5 s) ;
    //   - secret vérifié d'abord -> réponse immédiate, le corps n'est jamais touché.
    let patience = std::time::Duration::from_secs(3);
    let (statut, ecoule) = service
        .annoncer_un_corps_sans_l_envoyer(
            200_000,
            "un-mauvais-secret-de-quarante-huit-caracteres-ab",
            patience,
        )
        .await;
    println!("corps annoncé jamais envoyé, mauvais secret -> {statut:?} en {ecoule:?}");

    let statut = statut.expect(
        "aucune réponse avant l'expiration : le service attendait le corps, donc il le lisait \
         avant d'authentifier",
    );
    assert!(statut.contains("401"), "réponse inattendue : {statut}");
    assert!(
        ecoule < patience,
        "le refus a mis {ecoule:?} : le corps a été attendu avant l'authentification"
    );

    // Contrôle inverse : la protection de taille n'a pas été perdue en déplaçant
    // l'authentification. Avec le bon secret, un corps au-delà de la limite est bien refusé.
    let reponse = service.poster_volumineux(300 * 1024, harnais::SECRET).await;
    let code = reponse.status();
    let corps: serde_json::Value = reponse.json().await.expect("le refus est du JSON");
    println!("corps de 300 Kio, bon secret                -> {code} {corps}");
    assert_eq!(code, 413, "la limite de taille doit toujours mordre");
    assert_eq!(corps["code"], 2006);

    service.eteindre().await;
    println!("\nl'authentification précède la lecture du corps, et la limite tient toujours");
}
