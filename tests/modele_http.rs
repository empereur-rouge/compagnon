//! Le client HTTP du modèle, contre un serveur qui rejoue des réponses **observées**.
//!
//! # Ce que ces tests valent, et ce qu'ils ne valent pas
//!
//! Le serveur est faux ; les formes qu'il rejoue ne le sont pas. Chacune a été relevée sur un
//! vrai serveur compatible OpenAI (LM Studio servant un Mistral 24B), avec la commande
//! `compagnon modele essai`. Cette distinction est tout ce qui sépare ces tests d'un mock qui
//! valide un comportement inventé — et l'un d'eux a effectivement contredit ce que le code
//! supposait :
//!
//! | Forme relevée | Ce qu'on aurait supposé | Ce que le vrai serveur fait |
//! |---|---|---|
//! | chemin inexistant | `404` | **`200`** avec `{"error": …}` |
//! | modèle inconnu demandé | erreur | répond avec le modèle chargé, et le nomme |
//! | budget de jetons trop court | texte coupé | `content: ""`, `finish_reason: "length"` |
//!
//! Le premier faisait passer une URL fausse pour un incident passager, réessayé jusqu'à
//! épuisement des tentatives.

#![allow(clippy::expect_used)]

use std::time::Duration;

use compagnon::modele::http::{ClientHttp, ConfigModele};
use compagnon::modele::{ClientModele, ContexteConversation, ErreurModele, Panne, Role, Tour};
use compagnon::secret::Secret;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// La clé d'exemple. Aucun rendu du client ne doit la contenir.
const CLE: &str = "sk-une-cle-de-fournisseur-qui-ne-doit-jamais-sortir";

/// Le modèle demandé dans la configuration.
const MODELE_DEMANDE: &str = "mistral-small-3.2-24b";

/// Une configuration pointant vers le serveur donné.
fn config(base: &str) -> ConfigModele {
    ConfigModele {
        base: format!("{base}/v1"),
        cle: Secret::nouveau(CLE.to_owned()),
        modele: MODELE_DEMANDE.to_owned(),
        fournisseur: "fournisseur-de-test".to_owned(),
        jetons_max: 500,
        temperature: 0.85,
        delai: Duration::from_secs(5),
        // 0,14 € et 0,42 € le million : l'ordre de grandeur réel d'un 24B chez un hébergeur
        // serverless, pour que les montants des tests ressemblent à ceux de la production.
        prix_entree_eur_par_million: Decimal::new(14, 2),
        prix_sortie_eur_par_million: Decimal::new(42, 2),
    }
}

/// Monte un serveur qui rend `corps` avec le statut donné, et construit le client dessus.
async fn client_qui_recoit(statut: u16, corps: Value) -> (MockServer, ClientHttp) {
    let serveur = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(statut).set_body_json(corps))
        .mount(&serveur)
        .await;
    let client = ClientHttp::new(config(&serveur.uri())).expect("client constructible");
    (serveur, client)
}

/// Le contexte type : un prompt système validé, un message.
fn contexte() -> ContexteConversation {
    ContexteConversation {
        prompt_systeme: "Tu es Alix, 28 ans. Tu tutoies.".to_owned(),
        echanges: vec![Tour {
            role: Role::Utilisateur,
            texte: "Salut, ça va ?".to_owned(),
        }],
    }
}

/// La forme nominale, relevée telle quelle sur le vrai serveur.
fn reponse_nominale(contenu: &str, raison: &str) -> Value {
    json!({
        "id": "chatcmpl-ts0y04l3xsahf07tkgm8t",
        "object": "chat.completion",
        "model": "cognitivecomputations_dolphin3.0-r1-mistral-24b",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": contenu, "reasoning_content": "" },
            "finish_reason": raison
        }],
        "usage": { "prompt_tokens": 42, "completion_tokens": 38, "total_tokens": 80 }
    })
}

#[tokio::test]
async fn une_reponse_nominale_rend_le_texte_les_unites_et_le_modele_reel() {
    let (_serveur, client) =
        client_qui_recoit(200, reponse_nominale("Salut ! Ça va bien, et toi ?", "stop")).await;

    let reponse = client.repondre(&contexte()).await.expect("réponse");
    let cout = client.cout_eur(reponse.unites_entree, reponse.unites_sortie);

    println!("texte           : {}", reponse.texte);
    println!("modèle rendu    : {}", reponse.modele);
    println!("modèle demandé  : {MODELE_DEMANDE}");
    println!("unités          : {:?} / {:?}", reponse.unites_entree, reponse.unites_sortie);
    println!("tronquée        : {}", reponse.tronquee);
    println!("coût            : {cout:.6} €");

    assert_eq!(reponse.texte, "Salut ! Ça va bien, et toi ?");
    // Le modèle RENDU prime sur le modèle DEMANDÉ. Ce n'est pas un raffinement : mesuré sur le
    // vrai serveur, demander un modèle inconnu ne produit aucune erreur — il répond avec celui
    // qu'il a chargé. Inscrire le modèle demandé ferait comparer les coûts de deux versions
    // sur ce qu'on croyait appeler.
    assert_eq!(reponse.modele, "cognitivecomputations_dolphin3.0-r1-mistral-24b");
    assert_eq!(reponse.unites_entree, Some(42));
    assert_eq!(reponse.unites_sortie, Some(38));
    assert!(!reponse.tronquee);
    // 42 × 0,14/10⁶ + 38 × 0,42/10⁶ = 0,00000588 + 0,00001596
    assert_eq!(cout, Decimal::new(2184, 8));
}

#[tokio::test]
async fn le_prompt_systeme_part_en_premier_et_le_message_ensuite() {
    // Le format de fil est ce que ni le compilateur ni le type ne gardent : une inversion
    // d'ordre ferait parler le compagnon avec son prompt en guise de message d'utilisateur, et
    // aucun test de type ne l'attraperait.
    let serveur = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reponse_nominale("ok", "stop")))
        .mount(&serveur)
        .await;
    let client = ClientHttp::new(config(&serveur.uri())).expect("client");

    client.repondre(&contexte()).await.expect("réponse");

    let requetes = serveur.received_requests().await.expect("requêtes enregistrées");
    let corps: Value = serde_json::from_slice(&requetes[0].body).expect("corps JSON");
    println!("corps émis :\n{}", serde_json::to_string_pretty(&corps).expect("rendu"));

    assert_eq!(corps["model"], MODELE_DEMANDE);
    assert_eq!(corps["max_tokens"], 500);
    assert_eq!(corps["messages"][0]["role"], "system");
    assert_eq!(corps["messages"][0]["content"], "Tu es Alix, 28 ans. Tu tutoies.");
    assert_eq!(corps["messages"][1]["role"], "user");
    assert_eq!(corps["messages"][1]["content"], "Salut, ça va ?");
    assert_eq!(corps["messages"].as_array().expect("tableau").len(), 2);

    // La clé voyage en en-tête, pas dans l'URL — c'est ce qui distingue cette API de celle de
    // Telegram, et ce qui rend l'URL non secrète ici.
    let autorisation = requetes[0]
        .headers
        .get("authorization")
        .expect("en-tête d'autorisation")
        .to_str()
        .expect("en-tête lisible");
    println!("Authorization : Bearer <{} caractères>", autorisation.len() - "Bearer ".len());
    assert_eq!(autorisation, format!("Bearer {CLE}"));
    assert!(!requetes[0].url.as_str().contains(CLE), "la clé ne doit pas être dans l'URL");
}

#[tokio::test]
async fn un_200_annoncant_une_erreur_ne_se_rejoue_pas() {
    // LE cas qui a corrigé le code. Relevé mot pour mot sur le vrai serveur, sur un chemin
    // inexistant : statut 200, corps d'erreur. Sans cette distinction, la réponse se lisait
    // comme une génération vide — donc rejouable — et une URL fausse épuisait les tentatives
    // en affichant « le modèle n'a rien produit ».
    let (_serveur, client) = client_qui_recoit(
        200,
        json!({ "error": "Unexpected endpoint or method. (POST /v9/chat/completions)" }),
    )
    .await;

    let erreur = client.repondre(&contexte()).await.expect_err("doit échouer");
    println!("erreur : {erreur} — reprise : {}", erreur.merite_une_reprise());

    assert!(matches!(erreur, ErreurModele::RefusApplicatif));
    assert!(!erreur.merite_une_reprise(), "une cause permanente ne se rejoue pas");
    // Le message du fournisseur ne traverse pas : il reprend la requête, donc le prompt.
    assert!(!erreur.to_string().contains("v9"));
    assert!(!erreur.to_string().contains("endpoint"));
}

#[tokio::test]
async fn un_texte_vide_coupe_par_la_limite_se_distingue_d_un_modele_muet() {
    // Relevé sur le vrai modèle : quatre appels sur cinq à `max_tokens = 80` rendent
    // `content: ""` avec `finish_reason: "length"` — le raisonnement a mangé le budget.
    let (_serveur, tronque) = client_qui_recoit(200, reponse_nominale("", "length")).await;
    let erreur_tronquee = tronque.repondre(&contexte()).await.expect_err("doit échouer");

    let (_serveur2, muet) = client_qui_recoit(200, reponse_nominale("   \n ", "stop")).await;
    let erreur_vide = muet.repondre(&contexte()).await.expect_err("doit échouer");

    println!("budget épuisé  : {erreur_tronquee}");
    println!("modèle muet    : {erreur_vide}");

    assert!(matches!(erreur_tronquee, ErreurModele::Tronquee));
    assert!(matches!(erreur_vide, ErreurModele::Vide));
    // Les deux se rejouent — c'est le libellé qui diffère, et c'est lui qui envoie l'exploitant
    // vers `MODELE_JETONS_MAX` plutôt que vers une panne de modèle.
    assert!(erreur_tronquee.merite_une_reprise());
    assert!(erreur_vide.merite_une_reprise());
}

#[tokio::test]
async fn une_reponse_coupee_mais_non_vide_est_rendue_et_signalee() {
    let (_serveur, client) =
        client_qui_recoit(200, reponse_nominale("Je pensais justement à toi quand", "length")).await;

    let reponse = client.repondre(&contexte()).await.expect("réponse");
    println!("texte : {} | tronquée : {}", reponse.texte, reponse.tronquee);

    // Le texte part quand même : mieux vaut une phrase inachevée qu'un silence. Mais
    // l'appelant sait qu'elle l'est, ce qui lui laisse le choix de la couper proprement.
    assert_eq!(reponse.texte, "Je pensais justement à toi quand");
    assert!(reponse.tronquee);
}

#[tokio::test]
async fn les_refus_du_fournisseur_se_classent_par_leur_code() {
    let cas = [(429_u16, true), (500, true), (503, true), (400, false), (401, false), (403, false)];

    println!("Refus du fournisseur :");
    for (code, rejouable) in cas {
        let (_serveur, client) =
            client_qui_recoit(code, json!({ "error": { "message": "…", "type": "invalid_request" } })).await;
        let erreur = client.repondre(&contexte()).await.expect_err("doit échouer");
        println!(
            "  {code} → {erreur} — {}",
            if erreur.merite_une_reprise() { "on rejoue" } else { "on abandonne" }
        );

        assert!(matches!(erreur, ErreurModele::Refuse { code: recu } if recu == code));
        assert_eq!(erreur.merite_une_reprise(), rejouable);
        // Le corps d'une réponse d'erreur n'est jamais lu : il reprend la requête, donc le
        // prompt système, donc tout ce que le compagnon est.
        assert!(!erreur.to_string().contains("invalid_request"));
    }
}

#[tokio::test]
async fn un_fournisseur_muet_sur_les_unites_degrade_la_mesure_sans_perdre_la_reponse() {
    let sans_usage = json!({
        "model": "un-modele",
        "choices": [{ "message": { "content": "Coucou." }, "finish_reason": "stop" }]
    });
    let (_serveur, client) = client_qui_recoit(200, sans_usage).await;

    let reponse = client.repondre(&contexte()).await.expect("réponse");
    let cout = client.cout_eur(reponse.unites_entree, reponse.unites_sortie);
    println!("texte : {} | unités : {:?} | coût : {cout} €", reponse.texte, reponse.unites_entree);

    // La réponse arrive à l'utilisateur : un décompte manquant ne doit pas coûter un message.
    assert_eq!(reponse.texte, "Coucou.");
    assert_eq!(reponse.unites_entree, None);
    assert_eq!(cout, Decimal::ZERO, "un coût sous-estimé et visible, pas un trou");
}

#[tokio::test]
async fn un_fournisseur_trop_lent_est_abandonne_au_delai() {
    let serveur = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(reponse_nominale("trop tard", "stop"))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&serveur)
        .await;
    let mut configuration = config(&serveur.uri());
    configuration.delai = Duration::from_millis(300);
    let client = ClientHttp::new(configuration).expect("client");

    let debut = std::time::Instant::now();
    let erreur = client.repondre(&contexte()).await.expect_err("doit expirer");
    let mesure = debut.elapsed();
    println!("erreur : {erreur} — abandonné après {mesure:?}");

    assert!(matches!(erreur, ErreurModele::Injoignable(Panne::Delai)));
    assert!(mesure < Duration::from_secs(2), "le délai doit couper avant la réponse");
    assert!(erreur.merite_une_reprise());
}

#[tokio::test]
async fn aucune_erreur_du_client_ne_laisse_fuir_la_cle() {
    // Le client est construit sur un port fermé : `reqwest` produit une vraie erreur de
    // connexion, celle qui porte l'URL. Même si l'URL n'est pas secrète pour cette API, la
    // discipline vaut d'être éprouvée — elle perd sa force dès qu'elle admet une exception.
    let mut configuration = config("http://127.0.0.1:9");
    configuration.delai = Duration::from_millis(500);
    let client = ClientHttp::new(configuration).expect("client");

    let erreur = client.repondre(&contexte()).await.expect_err("le port 9 est fermé");
    let affichage = erreur.to_string();
    let debogage = format!("{erreur:?}");
    println!("Display : {affichage}");
    println!("Debug   : {debogage}");

    for rendu in [&affichage, &debogage] {
        assert!(!rendu.contains(CLE), "la clé fuit : {rendu}");
        assert!(!rendu.contains("sk-"), "un préfixe de clé fuit : {rendu}");
        assert!(!rendu.contains("127.0.0.1"), "l'hôte fuit : {rendu}");
    }

    // Et le `Debug` du client lui-même, celui qu'un journal de démarrage écrirait.
    let rendu_client = format!("{client:?}");
    println!("Debug du client : {rendu_client}");
    assert!(!rendu_client.contains(CLE), "la clé fuit dans le Debug du client");
    assert!(rendu_client.contains("masqué"));
}
