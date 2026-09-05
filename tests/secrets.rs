//! Les deux secrets du service ne sortent jamais, éprouvé sur le vrai chemin.
//!
//! # Pourquoi un fichier à part
//!
//! Ces tests ne vérifient pas une fonctionnalité : ils vérifient qu'une fonctionnalité ne fait
//! pas quelque chose. Cette classe de garantie se perd facilement — la version précédente du
//! test de fuite construisait la seule variante d'erreur qui ne pouvait pas fuir, et couvrait
//! donc exactement le complément du trou. Les regrouper ici les rend visibles.
//!
//! Les deux secrets, et par où ils pourraient partir :
//!
//! | Secret | Voyage dans | Fuite possible par |
//! |---|---|---|
//! | jeton du bot | l'URL de chaque appel sortant | une erreur `reqwest` journalisée |
//! | secret du webhook | l'en-tête de chaque appel entrant | une réponse d'erreur, un journal de proxy |

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::config::Config;
use compagnon::error::{ApiError, ErrorCode};
use compagnon::telegram::Canal;
use harnais::{FauxTelegram, JETON, SECRET, update_privee};

/// La partie du jeton qui ne doit jamais apparaître nulle part.
const PARTIE_SECRETE: &str = "AAExempleDeJetonQuiNeSertAAbsolumen";

#[tokio::test]
async fn une_panne_reseau_ne_laisse_pas_fuir_le_jeton() {
    // Le vrai chemin, pas un mock : le port 9 (discard) est fermé, `reqwest` produit donc une
    // vraie erreur de connexion — celle qui, avant correction, imprimait
    // « error sending request for url (http://.../bot<JETON>/getMe) ».
    let config = Config {
        jeton_bot: JETON.to_owned(),
        secret_webhook: SECRET.to_owned(),
        adresse_ecoute: "127.0.0.1:0".parse().expect("adresse littérale"),
        api_telegram: "http://127.0.0.1:9".to_owned(),
    };
    let canal = Canal::new(&config).expect("le client doit se construire");
    let erreur = canal
        .identite()
        .await
        .expect_err("le port 9 est fermé, l'appel doit échouer");

    let display = format!("{erreur}");
    let debug = format!("{erreur:?}");
    println!("Display : {display}");
    println!("Debug   : {debug}");

    for (nom, rendu) in [("Display", &display), ("Debug", &debug)] {
        assert!(
            !rendu.contains(PARTIE_SECRETE),
            "le jeton fuit dans le {nom} de l'erreur"
        );
        assert!(
            !rendu.contains("/bot"),
            "une URL de l'API fuit dans le {nom}"
        );
        assert!(!rendu.contains("127.0.0.1"), "l'hôte fuit dans le {nom}");
    }
    println!("\naucune trace du jeton ni de l'URL, dans aucun des deux rendus");
}

#[tokio::test]
async fn la_chaine_de_diagnostic_d_une_erreur_api_ne_traverse_pas_vers_une_url() {
    // `ApiError::diagnostic` parcourt toute la chaîne de `source()` et appelle `to_string()`
    // sur chaque maillon. Si une erreur d'envoi était attachée comme cause, l'URL ressortirait
    // par ce chemin-là même si le `Display` de premier niveau était propre.
    let config = Config {
        jeton_bot: JETON.to_owned(),
        secret_webhook: SECRET.to_owned(),
        adresse_ecoute: "127.0.0.1:0".parse().expect("adresse littérale"),
        api_telegram: "http://127.0.0.1:9".to_owned(),
    };
    let canal = Canal::new(&config).expect("le client doit se construire");
    let source = canal.identite().await.expect_err("le port 9 est fermé");

    let enveloppee = ApiError::avec_source(ErrorCode::Interne, "appel Telegram manqué", source);
    let diagnostic = enveloppee.diagnostic();
    println!("diagnostic complet : {diagnostic}");

    assert!(
        !diagnostic.contains(PARTIE_SECRETE),
        "le jeton fuit par la chaîne de causes"
    );
    assert!(
        !diagnostic.contains("/bot"),
        "une URL fuit par la chaîne de causes"
    );
    println!("la chaîne de causes est muette sur l'URL");
}

#[tokio::test]
async fn le_secret_du_webhook_n_apparait_dans_aucune_reponse() {
    let faux = FauxTelegram::demarrer().await;
    let service = harnais::demarrer(&faux).await;

    // Le bon secret, un mauvais secret, et une route inconnue : aucune des trois réponses ne
    // doit contenir le secret attendu, ni dire lequel des cas a échoué.
    let cas: Vec<(&str, String)> = vec![
        ("secret exact", SECRET.to_owned()),
        (
            "secret erroné",
            "un-autre-secret-de-quarante-huit-caracteres-abcd".to_owned(),
        ),
        ("secret vide", String::new()),
    ];

    for (nom, secret) in cas {
        let reponse = service
            .poster_avec_secret(&update_privee(950_001, "coucou"), &secret)
            .await;
        let statut = reponse.status();
        let corps = reponse.text().await.expect("corps lisible");
        println!("{nom:16} -> {statut} {corps}");
        assert!(
            !corps.contains(SECRET),
            "le secret attendu apparaît dans la réponse à « {nom} »"
        );
        assert!(
            !corps.contains("secret"),
            "la réponse à « {nom} » nomme le secret, ce qui renseigne un attaquant"
        );
    }

    service.eteindre().await;
    println!("\naucune réponse ne nomme ni ne divulgue le secret");
}
