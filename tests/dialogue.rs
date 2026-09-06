//! Ce que le service fait quand le modèle ne répond pas — et ce qu'il refuse de faire.
//!
//! # Pourquoi ces tests existent
//!
//! Les pannes d'un fournisseur de calcul sont rares, non reproductibles, et arrivent en
//! production. C'est la raison pour laquelle le worker reçoit un `ClientModele` au lieu de le
//! construire : un double les fabrique à la demande, et le service entier — webhook, file,
//! worker, base, Telegram — est éprouvé face à elles.
//!
//! Cinq situations, chacune avec une conduite différente :
//!
//! | Situation | Ce que fait le service |
//! |---|---|
//! | modèle qui expire | remet en file, puis prévient quand les reprises sont épuisées |
//! | fournisseur qui refuse (`401`) | prévient tout de suite : réessayer referait la même erreur |
//! | aucun compagnon actif | n'appelle pas le modèle, et le dit |
//! | prompt altéré hors processus | n'appelle **pas** le modèle |
//! | Telegram qui refuse après coup | inscrit quand même le coût déjà payé |

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::modele::ErreurModele;
use compagnon::modele::double::{ModeleDouble, modele_qui_expire};
use harnais::{FauxTelegram, UTILISATEUR, update_privee};

/// Un service prêt à converser : âge vérifié, compagnon actif.
async fn service_pret(faux: &FauxTelegram, modele: ModeleDouble) -> harnais::EnMarche {
    let service = harnais::demarrer_avec_modele(faux, modele).await;
    service.base().prete_a_converser(UTILISATEUR, "Alix").await;
    service
}

#[tokio::test]
async fn un_modele_qui_expire_est_rejoue_puis_la_personne_est_prevenue() {
    let faux = FauxTelegram::demarrer().await;
    // Expire à chaque appel : le scénario du double répète son dernier acte indéfiniment.
    let service = service_pret(&faux, modele_qui_expire()).await;

    service.poster(&update_privee(910_001, "tu es là ?")).await;

    // Ce qui doit arriver au bout : un message, et un seul. Le silence serait indiscernable
    // d'un bot mort — c'est la première friction que la carte des parcours signale.
    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("appels au modèle : {}", service.modele().appels());
    println!("message reçu :\n---\n{texte}\n---");

    assert!(
        texte.contains("Réessaie"),
        "la personne doit être prévenue, pas laissée devant un silence"
    );
    // Le modèle a bien été rejoué : trois prises de tâche, pas une.
    assert_eq!(
        service.modele().appels(),
        3,
        "la file borne les reprises à trois tentatives"
    );

    // Et le registre porte trois échecs : un appel qui rate est souvent facturé quand même,
    // et un coût invisible ne se retrouve pas.
    let lignes = service.base().attendre_registre(UTILISATEUR, 3).await;
    println!("registre : {lignes:?}");
    assert_eq!(lignes.len(), 3);
    assert!(lignes.iter().all(|(_, statut, _)| statut == "echec"));

    service.eteindre().await;
}

#[tokio::test]
async fn une_cle_invalide_ne_consomme_pas_les_reprises() {
    let faux = FauxTelegram::demarrer().await;
    // `401` : la clé est refusée. Rejouer referait exactement la même erreur, trois fois, en
    // retardant d'autant le moment où la personne apprend que ça ne marche pas.
    let service = service_pret(&faux, ModeleDouble::qui_echoue(ErreurModele::Refuse { code: 401 })).await;

    service.poster(&update_privee(910_002, "coucou")).await;
    let messages = faux.attendre("sendMessage", 1).await;
    println!("appels au modèle : {}", service.modele().appels());
    println!("message reçu : {}", messages[0]["text"]);

    assert_eq!(
        service.modele().appels(),
        1,
        "une cause permanente ne se rejoue pas"
    );
    assert!(
        messages[0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Réessaie")
    );

    // Rien du vocabulaire interne ne doit avoir traversé jusqu'à la personne.
    let recu = messages[0]["text"].as_str().unwrap_or_default().to_lowercase();
    for interdit in ["401", "modèle", "fournisseur", "clé"] {
        assert!(!recu.contains(interdit), "« {interdit} » a fuité jusqu'à l'utilisateur");
    }

    service.eteindre().await;
}

#[tokio::test]
async fn sans_compagnon_actif_le_modele_n_est_jamais_appele() {
    let faux = FauxTelegram::demarrer().await;
    // Âge vérifié, mais aucun compagnon : rien ne doit être facturé pour quelqu'un qui n'a
    // encore personne à qui parler.
    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(UTILISATEUR).await;

    service.poster(&update_privee(910_003, "il y a quelqu'un ?")).await;
    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("message reçu :\n---\n{texte}\n---");
    println!("appels au modèle : {}", service.modele().appels());

    assert_eq!(service.modele().appels(), 0, "aucun jeton ne doit être payé");
    assert!(texte.contains("assistant"));
    assert!(
        service.base().registre(UTILISATEUR).await.is_empty(),
        "aucune ligne de coût sans appel"
    );

    service.eteindre().await;
}

#[tokio::test]
async fn un_prompt_altere_hors_processus_ferme_l_acces_au_modele() {
    // LE test structurel de cette tranche. Le prompt système est le **seul** point de contrôle
    // de la modération : c'est lui qui a été examiné, et rien d'autre. Un texte modifié après
    // validation — une console psql, une restauration partielle, un script d'exploitation —
    // n'a franchi aucun contrôle, et il n'y a aucune raison de le donner au modèle.
    //
    // L'empreinte vit dans la même ligne que le texte : la console qui modifie l'un peut
    // modifier l'autre. C'est un contrôle de cohérence, pas un sceau — mais c'est déjà ce qui
    // manquait, et il attrape l'altération faite sans y penser.
    let faux = FauxTelegram::demarrer().await;
    let service = service_pret(&faux, ModeleDouble::qui_repond("je ne devrais pas parler")).await;

    let avant = service
        .base()
        .prompt_valide(service.base().personnage_de(UTILISATEUR).await)
        .await;
    println!("prompt validé, {} octets", avant.len());

    // L'altération qui atteint le contrôle d'empreinte : le texte change ET `valide_le` est
    // réhorodaté, ce qui empêche la révocation de 0008 de se déclencher. C'est le geste d'un
    // script d'exploitation ou d'une restauration partielle — le compagnon reste actif et
    // validé, et seule l'empreinte périmée le trahit.
    let modifie = service
        .base()
        .alterer_le_prompt_en_revalidant(UTILISATEUR)
        .await;
    println!("prompt altéré en revalidant → {modifie} ligne(s) modifiée(s)");

    service.poster(&update_privee(910_004, "dis-moi tout")).await;
    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("message reçu :\n---\n{texte}\n---");
    println!("appels au modèle : {}", service.modele().appels());

    assert_eq!(
        service.modele().appels(),
        0,
        "un prompt qui n'a franchi aucun contrôle ne part pas au modèle"
    );
    assert!(texte.contains("Réessaie"), "la personne est prévenue sans détail interne");
    assert!(
        service.base().registre(UTILISATEUR).await.is_empty(),
        "rien n'a été appelé, donc rien n'est facturé"
    );

    service.eteindre().await;
}

#[tokio::test]
async fn un_envoi_refuse_par_telegram_ne_fait_pas_repayer_la_generation() {
    // Le comportement d'avant, mesuré : la réponse était produite, l'envoi ratait, la tâche
    // repartait du début et **rappelait le modèle**. Trois générations facturées pour une panne
    // qui n'a rien à voir avec le modèle. Le test d'alors l'affirmait :
    //
    //     assert_eq!(lignes.len(), 3, "un appel payé par tentative");
    //
    // La réponse est désormais inscrite avant l'envoi, sans identifiant Telegram, ce qui la
    // rend renvoyable. Une reprise la retrouve et la renvoie telle quelle.
    let faux = FauxTelegram::demarrer().await;
    faux.casser_l_envoi().await;
    let service = service_pret(&faux, ModeleDouble::qui_repond("une réponse qui n'arrivera pas")).await;

    service.poster(&update_privee(910_005, "coucou")).await;

    // Trois tentatives d'envoi : la file va au bout de ses reprises.
    faux.attendre("sendMessage", 3).await;
    let lignes = service.base().attendre_registre(UTILISATEUR, 1).await;
    println!("appels au modèle : {}", service.modele().appels());
    println!("registre         : {lignes:?}");

    assert_eq!(
        service.modele().appels(),
        1,
        "la génération est payée une fois, pas une par tentative d'envoi"
    );
    assert_eq!(lignes.len(), 1, "une génération, une ligne de coût");
    assert_eq!(lignes[0].1, "ok", "le modèle a réussi ; c'est l'envoi qui a échoué");
    assert_eq!(lignes[0].2, "double-de-test", "le modèle rendu, pas celui demandé");

    // Et la réponse reste en attente en base, sans identifiant Telegram : elle a été produite,
    // elle n'a pas été reçue. C'est cette distinction que la mémoire de la phase 2 devra lire.
    let fil = service.base().messages_du_fil(UTILISATEUR).await;
    println!("fil en base : {fil:?}");
    assert_eq!(fil.len(), 2, "l'entrant et le sortant sont inscrits");

    service.eteindre().await;
}

#[tokio::test]
async fn une_reponse_bloquee_puis_debloquee_part_sans_nouvelle_generation() {
    // La contrepartie : quand Telegram redevient joignable, la réponse déjà produite part telle
    // quelle. C'est ce qui rend le renvoi utile plutôt que seulement moins coûteux.
    let faux = FauxTelegram::demarrer().await;
    // Un seul échec, puis Telegram redevient joignable.
    faux.casser_l_envoi_n_fois(1).await;
    let service = service_pret(&faux, ModeleDouble::qui_repond("Je pensais à toi.")).await;

    service.poster(&update_privee(910_008, "tu es là ?")).await;
    let messages = faux.attendre("sendMessage", 2).await;
    let dernier = messages
        .last()
        .and_then(|m| m["text"].as_str())
        .unwrap_or_default();
    println!("texte finalement délivré : {dernier}");
    println!("appels au modèle : {}", service.modele().appels());

    assert_eq!(dernier, "Je pensais à toi.", "c'est bien la réponse d'origine");
    assert_eq!(service.modele().appels(), 1, "et elle n'a été générée qu'une fois");

    service.eteindre().await;
}

#[tokio::test]
async fn reecrire_le_prompt_en_console_desactive_le_compagnon() {
    // La première barrière, posée par la migration 0008 : une réécriture du prompt sans
    // réémission de la validation révoque celle-ci et rabat le compagnon en `brouillon`.
    //
    // Le worker ne voit alors plus de compagnon actif — donc pas d'appel au modèle, et un
    // message qui invite à en recréer un plutôt qu'une excuse technique. C'est une issue
    // différente de celle du test précédent, pour un geste différent, et les deux comptent.
    let faux = FauxTelegram::demarrer().await;
    let service = service_pret(&faux, ModeleDouble::qui_repond("je ne devrais pas parler")).await;

    let modifie = service.base().alterer_le_prompt(UTILISATEUR).await;
    let (statut, valide) = service.base().etat_du_compagnon(UTILISATEUR).await;
    println!("prompt réécrit → {modifie} ligne(s) ; statut {statut}, validé {valide}");
    assert_eq!(statut, "brouillon", "la réécriture doit rabattre le compagnon");
    assert!(!valide, "et révoquer sa validation");

    service.poster(&update_privee(910_006, "et maintenant ?")).await;
    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("message reçu :\n---\n{texte}\n---");
    println!("appels au modèle : {}", service.modele().appels());

    assert_eq!(service.modele().appels(), 0, "un compagnon non actif ne parle pas");
    assert!(texte.contains("assistant"));
    assert!(
        service.base().registre(UTILISATEUR).await.is_empty(),
        "rien n'a été appelé, donc rien n'est facturé"
    );

    service.eteindre().await;
}

#[tokio::test]
async fn un_prompt_forge_avec_un_sceau_recalcule_est_refuse() {
    // LE test que le passage au HMAC existe pour rendre possible.
    //
    // La manœuvre est celle d'une console `psql` qui fait tout correctement : elle réécrit le
    // prompt, **recalcule le sceau**, et réémet la validation. Le compagnon reste actif, sa
    // validation est fraîche, et son sceau correspond au texte.
    //
    // Quand le sceau était un `sha256`, cela suffisait : tout ce qu'il fallait pour le forger
    // était dans la ligne, et le texte injecté — précisément la classe de contenu que la
    // modération existe pour empêcher — partait au modèle.
    //
    // La clé du HMAC vit dans l'environnement du processus. La base ne contient plus de quoi
    // fabriquer un sceau valide, et le worker le voit.
    let faux = FauxTelegram::demarrer().await;
    let service = service_pret(&faux, ModeleDouble::qui_repond("je ne devrais pas parler")).await;

    let forge = service.base().forger_le_prompt(UTILISATEUR).await;
    let (statut, valide) = service.base().etat_du_compagnon(UTILISATEUR).await;
    println!("prompt forgé → {forge} ligne(s) ; statut {statut}, validé {valide}");
    // Le compagnon a traversé les deux barrières précédentes : il est actif ET validé.
    assert_eq!(statut, "actif", "la manœuvre ne déclenche aucune révocation");
    assert!(valide, "et la validation paraît fraîche");

    service.poster(&update_privee(910_007, "raconte-moi")).await;
    let messages = faux.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!("message reçu :\n---\n{texte}\n---");
    println!("appels au modèle : {}", service.modele().appels());

    assert_eq!(
        service.modele().appels(),
        0,
        "un prompt scellé sans la clé n'atteint pas le modèle"
    );
    assert!(texte.contains("Réessaie"));
    assert!(
        service.base().registre(UTILISATEUR).await.is_empty(),
        "rien n'a été appelé, donc rien n'est facturé"
    );

    service.eteindre().await;
}
