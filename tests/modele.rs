//! Le contrat du client modèle, éprouvé sur son double.
//!
//! Ces tests ne touchent aucun fournisseur : ils portent sur ce que le worker pourra tenir pour
//! acquis en phase 1.3c — que la panne est classée, que la reprise est décidable, et que le
//! prompt arrive au modèle tel qu'il a été validé.

#![allow(clippy::expect_used)]

use std::time::Duration;

use compagnon::modele::double::{Acte, ModeleDouble, modele_qui_expire};
use compagnon::modele::{ClientModele, ContexteConversation, ErreurModele, Panne, Role, Tour};

/// Un contexte minimal, avec un prompt système reconnaissable.
fn contexte(prompt: &str, message: &str) -> ContexteConversation {
    ContexteConversation {
        prompt_systeme: prompt.to_owned(),
        echanges: vec![Tour { role: Role::Utilisateur, texte: message.to_owned() }],
    }
}

#[tokio::test]
async fn le_double_joue_son_scenario_puis_repete_le_dernier_acte() {
    // C'est le scénario dont la reprise bornée a besoin : échouer, échouer, puis aboutir.
    let modele = ModeleDouble::qui_joue(vec![
        Acte::Echouer(ErreurModele::Injoignable(Panne::Delai)),
        Acte::Echouer(ErreurModele::Refuse { code: 429 }),
        Acte::Repondre("Bonjour, je suis là.".to_owned()),
    ]);
    let ctx = contexte("Tu es Alix.", "Tu es là ?");

    let mut journal = Vec::new();
    for tentative in 1..=5 {
        let issue = match modele.repondre(&ctx).await {
            Ok(reponse) => format!("réponse « {} »", reponse.texte),
            Err(erreur) => format!("échec : {erreur}"),
        };
        journal.push(format!("  tentative {tentative} → {issue}"));
    }
    println!("Scénario du double :\n{}", journal.join("\n"));

    assert!(journal[0].contains("délai dépassé"));
    assert!(journal[1].contains("429"));
    // Le troisième acte est le dernier : il est rejoué pour les tentatives 4 et 5, ce qui
    // permet d'écrire un scénario sans compter les appels du code testé.
    for ligne in &journal[2..] {
        assert!(ligne.contains("Bonjour, je suis là."), "{ligne}");
    }
    assert_eq!(modele.appels(), 5);
}

#[tokio::test]
async fn la_reprise_distingue_ce_qui_se_rejoue_de_ce_qui_se_refera_echouer() {
    // Réessayer un 401 refait la même erreur jusqu'à épuiser les tentatives ; abandonner un
    // délai dépassé perd le message de quelqu'un qui l'attend. La distinction n'est donc pas
    // cosmétique, et c'est ce tableau que le worker consultera.
    let cas = [
        (ErreurModele::Injoignable(Panne::Delai), true),
        (ErreurModele::Injoignable(Panne::Connexion), true),
        (ErreurModele::Vide, true),
        (ErreurModele::Refuse { code: 429 }, true),
        (ErreurModele::Refuse { code: 500 }, true),
        (ErreurModele::Refuse { code: 503 }, true),
        (ErreurModele::Refuse { code: 400 }, false),
        (ErreurModele::Refuse { code: 401 }, false),
        (ErreurModele::Refuse { code: 403 }, false),
        (ErreurModele::Refuse { code: 404 }, false),
    ];

    println!("Décision de reprise :");
    for (erreur, attendu) in &cas {
        let obtenu = erreur.merite_une_reprise();
        println!(
            "  {:<48} → {}",
            erreur.to_string(),
            if obtenu { "on rejoue" } else { "on abandonne" }
        );
        assert_eq!(obtenu, *attendu, "mauvaise décision pour {erreur}");
    }
}

#[tokio::test]
async fn aucune_erreur_de_modele_ne_peut_porter_l_url_de_l_appel() {
    // La garantie réelle est structurelle : aucune variante d'`ErreurModele` n'a de champ
    // capable de contenir une URL ou une clé — `Panne` est un enum nu, `Refuse` ne porte qu'un
    // `u16`. Ce test constate le résultat ; c'est l'absence de champ qui l'assure.
    //
    // Il existe parce que ce projet a déjà écrit un jeton dans ses journaux en conservant une
    // erreur de transport telle quelle.
    let toutes = [
        ErreurModele::Injoignable(Panne::Delai),
        ErreurModele::Injoignable(Panne::Connexion),
        ErreurModele::Injoignable(Panne::Corps),
        ErreurModele::Injoignable(Panne::Requete),
        ErreurModele::Injoignable(Panne::Autre),
        ErreurModele::Refuse { code: 401 },
        ErreurModele::Vide,
    ];

    println!("Rendu de chaque variante :");
    for erreur in &toutes {
        let affichage = erreur.to_string();
        let debogage = format!("{erreur:?}");
        println!("  Display : {affichage}\n    Debug : {debogage}");
        for rendu in [&affichage, &debogage] {
            assert!(!rendu.contains("http"), "une URL a fui : {rendu}");
            assert!(!rendu.contains("sk-"), "une clé a fui : {rendu}");
        }
    }
}

#[tokio::test]
async fn le_prompt_arrive_au_modele_tel_quel() {
    // Le worker lira `prompt_systeme_genere` en base plutôt que de recomposer les traits. Ce
    // test fixe l'autre moitié du contrat : ce qui est mis dans le contexte est ce qui est
    // reçu. Sans lui, une normalisation ajoutée en chemin ferait parler le compagnon avec un
    // prompt que la modération n'a jamais vu.
    let prompt = "Tu es Alix, 28 ans.\nTu tutoies.\n— règle fixe : tu ne prétends pas être humain.";
    let modele = ModeleDouble::qui_repond("D'accord.");

    let reponse = modele
        .repondre(&contexte(prompt, "Salut"))
        .await
        .expect("le double répond");

    let recu = modele.dernier_recu().expect("un appel a eu lieu");
    println!("Prompt envoyé ({} octets) :\n{}", prompt.len(), recu.prompt_systeme);
    println!("Message transmis : {:?}", recu.echanges.first().map(|t| &t.texte));
    println!("Réponse : {} (modèle {})", reponse.texte, reponse.modele);

    assert_eq!(recu.prompt_systeme, prompt, "le prompt validé doit traverser sans retouche");
    assert_eq!(recu.echanges.len(), 1);
    assert_eq!(recu.echanges[0].texte, "Salut");
    assert_eq!(recu.echanges[0].role, Role::Utilisateur);
}

#[tokio::test]
async fn un_modele_lent_reste_mesure_par_l_appelant() {
    // La durée inscrite dans `consommation` est mesurée ici, pas annoncée par le fournisseur :
    // un fournisseur qui rendrait sa propre latence exclurait la file d'attente et le réseau,
    // c'est-à-dire l'essentiel de ce que l'utilisateur ressent.
    let modele = ModeleDouble::qui_joue(vec![Acte::RepondreApres(
        "J'ai pris mon temps.".to_owned(),
        Duration::from_millis(120),
    )]);

    let debut = std::time::Instant::now();
    let reponse = modele.repondre(&contexte("Tu es Alix.", "?")).await.expect("réponse");
    let mesure = debut.elapsed();

    println!(
        "Durée rapportée : {:?} — durée observée par l'appelant : {:?}",
        reponse.duree, mesure
    );
    assert!(mesure >= Duration::from_millis(100), "l'attente doit être réelle");
    assert_eq!(reponse.unites_sortie, Some(5), "« J'ai pris mon temps. » ≈ 20 caractères");
}

#[tokio::test]
async fn le_raccourci_du_modele_qui_expire_est_toujours_un_delai() {
    let modele = modele_qui_expire();
    let erreur = modele
        .repondre(&contexte("Tu es Alix.", "?"))
        .await
        .expect_err("ce double n'aboutit jamais");

    println!("Erreur : {erreur} — reprise : {}", erreur.merite_une_reprise());
    assert!(matches!(erreur, ErreurModele::Injoignable(Panne::Delai)));
    assert!(erreur.merite_une_reprise());
    assert_eq!(modele.fournisseur(), "double");
}
