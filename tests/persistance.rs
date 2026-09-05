//! Ce que la file en base apporte, éprouvé plutôt qu'annoncé.
//!
//! Quatre promesses de la phase 1.1, une par test :
//!
//! 1. une tâche non traitée **survit à l'arrêt** du service — la limite connue de la phase 0 ;
//! 2. un bail expiré est **repris**, donc la mort d'un worker ne perd rien ;
//! 3. l'ordre est tenu **dans** une conversation malgré quatre consommateurs concurrents ;
//! 4. la file est **bornée par utilisateur**, une table n'étant pas bornée par construction.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use std::time::Duration;

use harnais::{FauxTelegram, update_privee};

/// L'identifiant que porte [`harnais::update_privee`].
const UTILISATEUR: i64 = 42;

#[tokio::test]
async fn une_tache_non_traitee_survit_a_l_arret_du_service() {
    let faux = FauxTelegram::demarrer().await;
    // Telegram refuse : la tâche sera reprise, jamais close.
    faux.casser_l_envoi().await;

    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(UTILISATEUR).await;

    let reponse = service
        .poster(&update_privee(920_001, "message qui doit survivre"))
        .await;
    assert_eq!(reponse.status(), 200, "le webhook accuse réception");

    // Laisser le worker s'y casser les dents au moins une fois.
    faux.attendre("sendMessage", 1).await;

    let base = service.arreter().await;
    let etats = base.etats_de_la_file().await;
    println!("états de la file après l'arrêt : {etats:?}");
    let restants = base.taches_non_traitees().await;
    println!("tâches non traitées, sur disque : {restants}");
    assert_eq!(
        restants, 1,
        "la tâche doit survivre à l'arrêt — c'est toute la raison d'être de la file en base"
    );

    // Deuxième vie : un service repart sur la MÊME base, avec un Telegram qui répond.
    let faux2 = FauxTelegram::demarrer().await;
    let service2 = harnais::reprendre(&faux2, base).await;
    let messages = faux2.attendre("sendMessage", 1).await;
    let texte = messages[0]["text"].as_str().unwrap_or_default();
    println!(
        "repris après redémarrage : {}",
        texte.lines().next().unwrap_or_default()
    );
    assert!(
        texte.contains("message qui doit survivre"),
        "la tâche laissée par le service précédent doit être reprise"
    );
    service2.eteindre().await;
}

#[tokio::test]
async fn un_bail_expire_est_repris_par_un_autre_worker() {
    let faux = FauxTelegram::demarrer().await;
    // Un envoi lent tient la tâche « en cours » le temps qu'on périme son bail.
    faux.ralentir_l_envoi(Duration::from_millis(600)).await;

    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(UTILISATEUR).await;
    service
        .poster(&update_privee(930_001, "bail à reprendre"))
        .await;

    // Attendre que la tâche soit prise, puis faire comme si le worker était mort en la tenant.
    faux.attendre("sendMessage", 1).await;
    let perimes = service.base().perimer_les_baux().await;
    println!("baux périmés de force : {perimes}");
    assert_eq!(perimes, 1, "une tâche doit être en cours à cet instant");

    // Un autre worker doit la reprendre : un second `sendMessage` part.
    let messages = faux.attendre("sendMessage", 2).await;
    println!("envois observés après péremption : {}", messages.len());
    assert!(
        messages.len() >= 2,
        "un bail expiré doit rendre la tâche prenable, sinon la mort d'un worker la perd"
    );
    service.eteindre().await;
}

#[tokio::test]
async fn l_ordre_est_tenu_dans_une_conversation_malgre_les_workers_concurrents() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(UTILISATEUR).await;

    // Le point éprouvé : quatre consommateurs tournent en parallèle, et pourtant les messages
    // d'une même personne ressortent dans l'ordre. Ce n'est pas le worker qui l'assure — il
    // n'a aucune synchronisation — mais la requête de prise, qui écarte tout utilisateur déjà
    // servi ailleurs. Sans elle, ce test échouerait de façon intermittente.
    const COMBIEN: i64 = 20;
    for numero in 0..COMBIEN {
        let reponse = service
            .poster(&update_privee(940_000 + numero, &format!("rang {numero}")))
            .await;
        assert_eq!(reponse.status(), 200);
    }

    let messages = faux.attendre("sendMessage", COMBIEN as usize).await;
    let rangs: Vec<usize> = messages
        .iter()
        .filter_map(|m| m["text"].as_str())
        .filter_map(|t| {
            t.split("rang ")
                .nth(1)?
                .split_whitespace()
                .next()?
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
        .collect();
    println!("ordre de sortie : {rangs:?}");
    let attendu: Vec<usize> = (0..COMBIEN as usize).collect();
    assert_eq!(rangs, attendu, "l'ordre a été rompu dans une conversation");
    service.eteindre().await;
}

#[tokio::test]
async fn la_file_est_bornee_par_utilisateur() {
    let faux = FauxTelegram::demarrer().await;
    // Assez lent pour que rien ne se vide pendant qu'on remplit.
    faux.ralentir_l_envoi(Duration::from_secs(30)).await;

    let service = harnais::demarrer(&faux).await;
    service.base().verifier_age(UTILISATEUR).await;

    // Une table n'est pas bornée par construction, contrairement au canal de la phase 0 : sans
    // borne, un émetteur en rafale transforme un afflux en disque plein.
    let borne = 32;
    let mut acceptes = 0;
    let mut refuses = 0;
    let mut premier_refus = None;
    for numero in 0..borne + 5 {
        let statut = service
            .poster(&update_privee(
                950_000 + numero,
                &format!("rafale {numero}"),
            ))
            .await
            .status();
        if statut == 200 {
            acceptes += 1;
        } else {
            refuses += 1;
            premier_refus.get_or_insert((numero, statut));
        }
    }
    println!("borne {borne} : {acceptes} acceptés, {refuses} refusés");
    println!("premier refus : {premier_refus:?}");
    assert!(refuses > 0, "la file doit finir par refuser");
    let (_, statut) = premier_refus.expect("il y a eu un refus");
    assert_eq!(
        statut, 503,
        "le refus doit demander à Telegram de rejouer, pas déclarer une erreur définitive"
    );
    service.eteindre().await;
}
