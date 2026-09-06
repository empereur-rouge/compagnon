//! La boucle de bout en bout : Telegram appelle, le service répond à Telegram.
//!
//! Chaque test part d'un service réellement démarré sur une socket, et n'observe que ce que le
//! faux Telegram a **reçu** — jamais l'état interne du service. C'est ce qui rend ces tests
//! indépendants de l'implémentation : la phase 1 remplacera l'écho par un modèle, et ces tests
//! continueront de dire la vérité sur le transport.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::modele::double::ModeleDouble;
use harnais::{FauxTelegram, update_privee};

#[tokio::test]
async fn un_message_prive_traverse_tout_le_circuit_et_revient_du_modele() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;
    println!("service à l'écoute sur {}", service.adresse);
    // Deux conditions, chacune éprouvée par son propre test : l'âge vérifié, et un compagnon
    // actif dont le prompt a passé la modération.
    let compagnon = service
        .base()
        .prete_a_converser(harnais::UTILISATEUR, "Alix")
        .await;
    println!("compagnon actif : {compagnon}");

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
    assert_eq!(
        texte,
        harnais::REPONSE_DU_DOUBLE,
        "c'est le modèle qui répond, plus l'écho"
    );

    // Et surtout : ce que le modèle a REÇU. C'est la propriété qu'aucune assertion sur la
    // réponse ne pourrait voir — le worker lit `prompt_systeme_genere`, celui que la modération
    // a approuvé, au lieu de recomposer les traits.
    let demande = service.modele().dernier_recu().expect("le modèle a été appelé");
    println!("\nprompt système envoyé au modèle :\n---\n{}\n---", demande.prompt_systeme);
    println!("échanges transmis : {:?}", demande.echanges.iter().map(|t| &t.texte).collect::<Vec<_>>());
    assert!(demande.prompt_systeme.contains("Alix"), "le prompt doit être celui de CE compagnon");
    assert_eq!(demande.echanges.len(), 1);
    assert_eq!(demande.echanges[0].texte, "salut, tu fais quoi ?");

    // Le prompt reçu est EXACTEMENT celui que la base a validé, à l'octet près.
    let valide = service.base().prompt_valide(compagnon).await;
    assert_eq!(demande.prompt_systeme, valide, "aucune retouche entre la base et le modèle");

    // L'indication d'activité part avant la réponse : c'est ce qui donne l'illusion, et elle
    // compte davantage qu'avant — l'écho partait en 50 ms, un modèle met des secondes.
    let actions = faux.attendre("sendChatAction", 1).await;
    println!("action affichée : {}", actions[0]["action"]);
    assert_eq!(actions[0]["action"], "typing");

    // Le fil est inscrit des deux côtés : ce que la personne a écrit, et ce qui lui a répondu.
    // L'attente n'est pas du confort : le worker envoie à Telegram PUIS inscrit, pour qu'une
    // ligne dans `messages` signifie « la personne l'a reçu ». Lire aussitôt après l'envoi
    // tombait parfois entre les deux.
    let echanges = service
        .base()
        .attendre_messages(harnais::UTILISATEUR, 2)
        .await;
    println!("\nfil en base :");
    for (role, contenu) in &echanges {
        println!("  {role:12} {contenu}");
    }
    assert_eq!(echanges.len(), 2, "l'entrant et le sortant doivent être inscrits");
    assert_eq!(echanges[0], ("utilisateur".to_owned(), "salut, tu fais quoi ?".to_owned()));
    assert_eq!(echanges[1].0, "personnage");

    // Et le coût est au registre, imputé à la bonne personne.
    let lignes = service
        .base()
        .attendre_registre(harnais::UTILISATEUR, 1)
        .await;
    println!("\nregistre des coûts : {lignes:?}");
    assert_eq!(lignes.len(), 1, "un appel, une ligne");
    assert_eq!(lignes[0].0, "message");
    assert_eq!(lignes[0].1, "ok");

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
    // Le modèle produit une réponse plus longue que ce que Telegram accepte en un message.
    // Ce n'est plus l'enrobage de l'écho qui fait franchir la limite mais le modèle lui-même,
    // ce qui est aussi le cas réel : rien n'empêche un modèle de rendre six mille caractères.
    let motif = "Elle repose sa tasse et te regarde sans rien dire. ";
    let longue_reponse: String = motif.chars().cycle().take(6000).collect();
    let service =
        harnais::demarrer_avec_modele(&faux, ModeleDouble::qui_repond(&longue_reponse)).await;
    service.base().prete_a_converser(harnais::UTILISATEUR, "Alix").await;
    println!(
        "réponse du modèle : {} unités UTF-16 (plafond sortant : 4096)",
        harnais::longueur_utf16(&longue_reponse)
    );

    let reponse = service.poster(&update_privee(900_005, "raconte")).await;
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
async fn l_extinction_ne_perd_rien_de_ce_qui_a_ete_accepte() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(harnais::UTILISATEUR).await;

    // Ce que l'extinction garantit a CHANGÉ, et dans le bon sens.
    //
    // Avec la file en mémoire, la seule façon de ne rien perdre était de la vider entièrement
    // avant de rendre la main — ce qui, face à un Telegram lent, pouvait dépasser le sursis de
    // Docker et perdre le reste en silence.
    //
    // Avec la file en base, ce qui n'a pas été traité SURVIT à l'arrêt. L'extinction n'a donc
    // plus à tout vider : elle doit seulement finir les tâches en cours, pour qu'aucune ne soit
    // reprise au bail et répondue deux fois. La garantie éprouvée ici est donc :
    // « accepté + répondu + resté en file == accepté », et non plus « tout est répondu ».
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

    // `arreter` et non `eteindre` : la base doit survivre à l'arrêt pour qu'on puisse compter
    // ce qui y reste — c'est exactement ce que ce test éprouve.
    let base = service.arreter().await;

    let repondus = faux.corps("sendMessage").await.len() as i64;
    let restants = base.taches_non_traitees().await;

    println!("répondus avant l'arrêt : {repondus}");
    println!("restés en file          : {restants}");
    println!(
        "total                   : {} (attendu {COMBIEN})",
        repondus + restants
    );
    assert_eq!(
        repondus + restants,
        COMBIEN,
        "l'extinction a perdu {} message(s)",
        COMBIEN - (repondus + restants)
    );

    // Et ce qui est parti l'est dans l'ordre : la sérialisation par utilisateur tient malgré
    // les quatre consommateurs concurrents.
    let messages = faux.corps("sendMessage").await;
    for (rang, message) in messages.iter().enumerate() {
        let texte = message["text"].as_str().unwrap_or_default();
        assert!(
            texte.contains(&format!("message {rang}")),
            "le message {rang} n'est pas à sa place — l'ordre par conversation a été rompu"
        );
    }
    println!("ordre respecté sur les {} réponses parties", messages.len());
    base.detruire().await;
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
    println!("GET /health -> {sante:?}");

    assert_eq!(sante.statut, "ok");
    assert_eq!(sante.version, env!("CARGO_PKG_VERSION"));
    assert!(sante.base_repond, "la base doit répondre");
    assert_eq!(
        sante.taches_en_attente,
        Some(0),
        "au repos, rien ne doit attendre"
    );
    assert_eq!(
        sante.workers,
        compagnon::worker::WORKERS,
        "la sonde doit annoncer les consommateurs qui tournent"
    );
    println!(
        "base_repond={} taches_en_attente={:?} workers={}",
        sante.base_repond, sante.taches_en_attente, sante.workers
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

#[tokio::test]
async fn sans_verification_d_age_le_moteur_reste_ferme() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // AUCUN appel à `verifier_age` : c'est tout l'objet de ce test.
    //
    // La barrière d'âge est la fonctionnalité vedette de cette phase, et elle n'était présente
    // dans la suite que comme *condition* — quatre tests l'écartaient en préambule, aucun ne
    // l'éprouvait. Sa seule couverture vérifiait qu'une constante contient un morceau
    // d'elle-même : elle serait passée si la barrière avait été retirée.
    let reponse = service
        .poster(&update_privee(960_001, "salut, on discute ?"))
        .await;
    assert_eq!(
        reponse.status(),
        200,
        "la mise à jour est acquittée malgré le refus"
    );

    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("réponse à un utilisateur non vérifié :\n---\n{texte}\n---");

    // Ce qu'il DOIT recevoir : un message qui dit ce qui manque. Un silence serait
    // indiscernable d'une panne — c'est la première friction que la carte des parcours signale.
    assert!(
        texte.contains("vérification d'âge"),
        "le refus doit nommer ce qui manque, pas se taire"
    );
    // Et ce qu'il ne doit PAS recevoir : son propre message en écho.
    assert!(
        !texte.contains("salut, on discute ?"),
        "le moteur ne doit pas avoir tourné"
    );

    // Et le modèle n'a PAS été appelé : un refus d'âge ne doit rien coûter.
    println!("appels au modèle : {}", service.modele().appels());
    assert_eq!(service.modele().appels(), 0, "aucun jeton ne doit être payé pour un refus");

    // Puis la barrière se lève, et le même utilisateur obtient une vraie réponse.
    service.base().prete_a_converser(harnais::UTILISATEUR, "Alix").await;
    service
        .poster(&update_privee(960_002, "et maintenant ?"))
        .await;
    let messages = faux.attendre("sendMessage", 2).await;
    let texte = messages[1]["text"].as_str().unwrap_or_default();
    println!("après vérification :\n---\n{texte}\n---");
    assert_eq!(
        texte,
        harnais::REPONSE_DU_DOUBLE,
        "une fois l'âge vérifié, le moteur doit répondre"
    );
    assert_eq!(service.modele().appels(), 1, "et le modèle est appelé exactement une fois");

    service.eteindre().await;
}
