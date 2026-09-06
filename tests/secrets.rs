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
//! | mot de passe de la base | `DATABASE_URL` | le `Debug` de `Config`, les journaux de démarrage |

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::error::{ApiError, ErrorCode};
use compagnon::telegram::Canal;
use harnais::{FauxTelegram, SECRET, update_privee};

/// La partie du jeton qui ne doit jamais apparaître nulle part.
///
/// **Dérivée** du jeton de `fixtures`, jamais recopiée. Une copie littérale a existé ici : le
/// jour où `JETON` aurait changé, les quatre `assert!(!rendu.contains(…))` seraient devenus
/// vides de sens **en continuant de passer** — un test de non-fuite qui ne teste plus rien.
/// `src/config.rs` avait déjà tiré la leçon et dérivait, lui.
fn partie_secrete() -> &'static str {
    compagnon::fixtures::JETON
        .split_once(':')
        .expect("le jeton d'exemple a la forme <id>:<secret>")
        .1
}

#[tokio::test]
async fn une_panne_reseau_ne_laisse_pas_fuir_le_jeton() {
    // Le vrai chemin, pas un mock : le port 9 (discard) est fermé, `reqwest` produit donc une
    // vraie erreur de connexion — celle qui, avant correction, imprimait
    // « error sending request for url (http://.../bot<JETON>/getMe) ».
    let config = compagnon::fixtures::config_de_test("http://127.0.0.1:9");
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
            !rendu.contains(partie_secrete()),
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
    let config = compagnon::fixtures::config_de_test("http://127.0.0.1:9");
    let canal = Canal::new(&config).expect("le client doit se construire");
    let source = canal.identite().await.expect_err("le port 9 est fermé");

    let enveloppee = ApiError::avec_source(ErrorCode::Interne, "appel Telegram manqué", source);
    let diagnostic = enveloppee.diagnostic();
    println!("diagnostic complet : {diagnostic}");

    assert!(
        !diagnostic.contains(partie_secrete()),
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

#[test]
fn aucune_forme_d_url_de_base_ne_laisse_fuir_le_mot_de_passe() {
    // Ce test existe parce que la version précédente de `masquer_url` échouait OUVERTE sur deux
    // de ces formes, en documentant l'inverse. Elle découpait la chaîne sur « :// » puis sur
    // « @ », ce qui est une grammaire devinée — et se tromper sur une forme à laquelle on n'a
    // pas pensé donne ici un mot de passe dans un journal.
    //
    // Les formes ci-dessous ne sont pas imaginées : ce sont celles que `sqlx` accepte, prises
    // de ses propres tests d'analyse.
    const SECRET_BASE: &str = "MotDePasseQuiNeDoitJamaisSortir";

    let formes = [
        (
            "nominale",
            format!("postgres://compagnon:{SECRET_BASE}@base:5432/compagnon"),
        ),
        (
            "mot de passe en paramètre",
            format!("postgres:///?password={SECRET_BASE}"),
        ),
        (
            "arobase dans l'utilisateur",
            format!("postgres://user@host:{SECRET_BASE}@host:5432/base"),
        ),
        ("schéma long", format!("postgresql://u:{SECRET_BASE}@h/d")),
        ("illisible", format!("pas une url du tout {SECRET_BASE}")),
    ];

    for (nom, url) in &formes {
        let rendu = compagnon::config::masquer_url(url);
        println!("{nom:28} -> {rendu}");
        assert!(
            !rendu.contains(SECRET_BASE),
            "le mot de passe fuit sur la forme « {nom} »"
        );
    }
    println!(
        "\nles {} formes sont muettes sur le mot de passe",
        formes.len()
    );
}

#[test]
fn le_debug_de_la_config_ne_montre_pas_le_mot_de_passe_de_la_base() {
    // La même garantie, mais sur le chemin par lequel elle sortirait vraiment : `Config` est
    // journalisée en entier au démarrage, par `servir` comme par `ecouter`.
    const SECRET_BASE: &str = "MotDePasseDeLaBase";
    let config = compagnon::fixtures::config_de_test_sur(
        "https://api.telegram.org",
        &format!("postgres://compagnon:{SECRET_BASE}@base:5432/compagnon"),
    );
    let rendu = format!("{config:?}");
    println!("Debug de Config :\n  {rendu}");

    assert!(
        !rendu.contains(SECRET_BASE),
        "le mot de passe de la base fuit dans le Debug"
    );
    // Et ce qui sert au diagnostic doit rester lisible : sans l'hôte ni la base, la ligne de
    // journal ne répondrait pas à la première question d'un incident.
    assert!(rendu.contains("base:5432"), "l'hôte doit rester visible");
    assert!(
        rendu.contains("compagnon"),
        "l'utilisateur et la base doivent rester visibles"
    );
}

#[test]
fn le_debug_du_canal_masque_le_jeton_qu_il_porte() {
    // `Canal` a longtemps **refusé** de dériver `Debug`, précisément parce que sa racine porte
    // le jeton. Une interdiction ne protège que ce qu'elle couvre : elle n'empêchait pas
    // d'écrire `format!("{:?}", canal.racine)` un cran plus bas, ce qu'aucun compilateur
    // n'aurait signalé.
    //
    // Les deux champs secrets étant devenus des `Secret`, la dérivation est désormais le rendu
    // masqué. Ce test constate ce qu'elle produit — c'est le rendu qu'un `tracing::debug!` sur
    // l'état partagé écrirait dans les journaux.
    let config = compagnon::fixtures::config_de_test("https://api.telegram.org");
    let canal = Canal::new(&config).expect("le client doit se construire");

    let rendu = format!("{canal:?}");
    println!("Debug de Canal :\n  {rendu}");

    assert!(!rendu.contains(partie_secrete()), "le jeton fuit dans le Debug du canal");
    assert!(!rendu.contains(SECRET), "le secret du webhook fuit dans le Debug du canal");
    // La racine entière est masquée, pas seulement sa partie secrète : c'est l'URL complète
    // qui a fui la première fois, et « api.telegram.org/bot123456789 » identifie déjà le bot.
    assert!(!rendu.contains("api.telegram.org"), "l'URL de l'API fuit dans le Debug du canal");
    assert!(rendu.contains("masqué"), "le rendu doit dire qu'il masque quelque chose");
}
