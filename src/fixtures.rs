//! Valeurs d'exemple partagées par les tests unitaires **et** les tests d'intégration.
//!
//! # Pourquoi ce module existe, et pourquoi il est dans `src/`
//!
//! Le jeton d'exemple, le secret de webhook et la construction d'une [`Config`] étaient
//! recopiés dans six fichiers, répartis sur deux cibles de compilation. Ajouter un champ à
//! `Config` cassait donc six endroits, et une faute de frappe dans l'une des copies du jeton
//! faisait échouer un test loin de sa cause — une revue l'avait signalé, et l'ajout de
//! `url_base` l'a confirmé le jour même.
//!
//! `tests/harnais/` appartient à une autre cible que `src/` : il ne peut pas atteindre un
//! module `#[cfg(test)]`. La réutilisation doit donc descendre dans la bibliothèque, derrière
//! la caractéristique `fixtures`, activée pour les tests et absente du binaire livré.

// Ce module n'est compilé que pour les tests : un `expect` sur une adresse littérale y est le
// bon outil, là où le code de production doit rendre une erreur.
#![allow(clippy::expect_used)]

use crate::config::Config;

/// Jeton d'exemple, de forme valide et sans aucune valeur réelle.
///
/// La partie secrète fait exactement 35 caractères, comme l'exige la validation : la raccourcir
/// ferait échouer des tests sans rapport avec ce qu'ils éprouvent.
pub const JETON: &str = "123456789:AAExempleDeJetonQuiNeSertAAbsolumen";

/// Secret de webhook d'exemple, dans les bornes acceptées par Telegram.
pub const SECRET: &str = "un-secret-de-quarante-huit-caracteres-exactement";

/// URL de la base de test, surchargeable par `DATABASE_URL_TEST`.
///
/// Le port 5433 et non 5432 : un PostgreSQL de développement déjà installé sur la machine ne
/// doit jamais être atteint par une suite de tests qui crée et détruit des bases.
#[must_use]
pub fn url_base_test() -> String {
    std::env::var("DATABASE_URL_TEST")
        .unwrap_or_else(|_| "postgres://compagnon:test@127.0.0.1:5433/compagnon_test".to_owned())
}

/// Une configuration complète, pointant vers `api` pour Telegram et `base` pour PostgreSQL.
#[must_use]
pub fn config_de_test(api: &str, base: &str) -> Config {
    Config {
        jeton_bot: JETON.to_owned(),
        secret_webhook: SECRET.to_owned(),
        // Port zéro : le système attribue un port libre, ce qui permet à plusieurs tests de
        // tourner en parallèle sans se disputer une adresse fixe.
        adresse_ecoute: "127.0.0.1:0"
            .parse()
            .expect("adresse littérale toujours valide"),
        url_base: base.to_owned(),
        api_telegram: api.to_owned(),
    }
}
