//! Configuration lue dans l'environnement, et validée **au démarrage**.
//!
//! # Pourquoi tout valider au démarrage
//!
//! Un jeton mal collé, un secret vide, une adresse d'écoute mal orthographiée : ces fautes se
//! commettent au déploiement. Découvertes au premier message, elles se traduisent par une
//! personne qui écrit dans le vide et un journal que personne ne lit. Découvertes au
//! démarrage, elles se traduisent par un conteneur qui refuse de partir, avec le nom de la
//! variable fautive — ce que l'exploitant voit immédiatement.
//!
//! La vérification va jusqu'à la **forme** des secrets, pas seulement leur présence :
//! `TELEGRAM_BOT_TOKEN=changeme` passerait un test de présence et échouerait au premier appel.
//!
//! # Pourquoi l'environnement, et pas un fichier
//!
//! Ce qui vit ici est entièrement secret. Un fichier de configuration finit dans une
//! sauvegarde, dans un `docker cp`, dans une capture d'écran. Le reste — les personnages, les
//! quotas — arrivera en base à partir de la phase 1, pas ici.

use std::fmt;
use std::net::SocketAddr;

/// Racine de l'API Telegram, quand `API_TELEGRAM` n'est pas positionnée.
const API_TELEGRAM_DEFAUT: &str = "https://api.telegram.org";

/// Adresse d'écoute par défaut : toutes les interfaces, port 8080.
///
/// Le service n'est pas censé être joignable directement — Caddy termine le TLS devant lui et
/// le port n'est pas publié par `compose.yaml`.
const ADRESSE_ECOUTE_DEFAUT: &str = "0.0.0.0:8080";

/// Longueur minimale exigée du secret de webhook.
///
/// Telegram accepte de 1 à 256 caractères. Un caractère serait accepté par Telegram et
/// deviné en une seconde : le webhook est une adresse publique, et ce secret est la *seule*
/// chose qui distingue Telegram de n'importe qui d'autre.
const SECRET_LONGUEUR_MIN: usize = 32;

/// Longueur maximale acceptée par Telegram pour le secret de webhook.
const SECRET_LONGUEUR_MAX: usize = 256;

/// Nombre de caractères de la partie secrète d'un jeton de bot (après les deux-points).
const JETON_PARTIE_SECRETE: usize = 35;

/// Ce qui a empêché la configuration d'être lue.
#[derive(Debug, thiserror::Error)]
pub enum ErreurConfig {
    /// La variable est absente de l'environnement, ou vide.
    #[error("variable d'environnement absente ou vide : {0}")]
    Absente(&'static str),

    /// La variable est présente mais sa valeur ne convient pas.
    ///
    /// La valeur fautive n'apparaît **pas** dans le message : ces variables portent des
    /// secrets, et ce message finit dans les journaux du conteneur.
    #[error("{variable} : {raison}")]
    Invalide {
        /// Nom de la variable en cause.
        variable: &'static str,
        /// Ce qui ne va pas, sans jamais citer la valeur.
        raison: String,
    },
}

/// Tout ce dont le service a besoin pour démarrer.
///
/// [`fmt::Debug`] est écrit à la main et masque les deux secrets : une structure de
/// configuration finit tôt ou tard dans un `tracing::debug!`, et un jeton de bot dans les
/// journaux vaut une reprise complète du bot.
///
/// Ne dérive pas `Clone`, et l'omission est délibérée : la structure porte deux secrets, elle
/// est lue une fois au démarrage, et rien n'a besoin d'en faire une copie. Une dérivation
/// gratuite laisserait un jour quelqu'un en semer des exemplaires sur le tas.
pub struct Config {
    /// Jeton donné par `@BotFather`. Secret.
    pub jeton_bot: String,
    /// Secret partagé renvoyé par Telegram dans chaque appel de webhook. Secret.
    pub secret_webhook: String,
    /// Adresse sur laquelle le service écoute.
    pub adresse_ecoute: SocketAddr,
    /// Racine de l'API Telegram, sans barre oblique finale.
    pub api_telegram: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            // L'identifiant du bot (avant les deux-points) n'est pas secret et identifie
            // l'instance dans les journaux ; la partie qui suit ne sort jamais.
            .field(
                "jeton_bot",
                &format_args!("{}:<masqué>", self.identifiant_bot()),
            )
            .field(
                "secret_webhook",
                &format_args!("<masqué, {} caractères>", self.secret_webhook.len()),
            )
            .field("adresse_ecoute", &self.adresse_ecoute)
            .field("api_telegram", &self.api_telegram)
            .finish()
    }
}

impl Config {
    /// Lit et valide la configuration depuis l'environnement du processus.
    ///
    /// Charge d'abord un `.env` s'il existe, pour le confort du développement ; en production
    /// les variables viennent de `compose.yaml` et le fichier est absent.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurConfig`] à la première variable absente ou mal formée, en la nommant.
    pub fn depuis_environnement() -> Result<Self, ErreurConfig> {
        // Une absence de `.env` n'est pas une erreur : c'est le cas normal en production.
        let _ = dotenvy::dotenv();

        // Les validateurs ne rendent pas la valeur : elle est déjà possédée ici. La rendre
        // ferait une seconde copie du secret sur le tas, dans un module dont toute la thèse
        // est qu'un secret ne se disperse pas.
        let jeton_bot = lire("TELEGRAM_BOT_TOKEN")?;
        valider_jeton_bot(&jeton_bot)?;
        let secret_webhook = lire("TELEGRAM_SECRET_WEBHOOK")?;
        valider_secret(&secret_webhook)?;

        let adresse_brute = lire_ou("ADRESSE_ECOUTE", ADRESSE_ECOUTE_DEFAUT);
        let adresse_ecoute =
            adresse_brute
                .parse::<SocketAddr>()
                .map_err(|erreur| ErreurConfig::Invalide {
                    variable: "ADRESSE_ECOUTE",
                    raison: format!("adresse d'écoute illisible ({erreur})"),
                })?;

        let api_telegram = lire_ou("API_TELEGRAM", API_TELEGRAM_DEFAUT);
        let api_telegram = api_telegram.trim_end_matches('/').to_owned();
        if !api_telegram.starts_with("http://") && !api_telegram.starts_with("https://") {
            return Err(ErreurConfig::Invalide {
                variable: "API_TELEGRAM",
                raison: "doit commencer par http:// ou https://".to_owned(),
            });
        }

        Ok(Self {
            jeton_bot,
            secret_webhook,
            adresse_ecoute,
            api_telegram,
        })
    }

    /// L'identifiant numérique du bot, extrait du jeton.
    ///
    /// Il n'est pas secret — il apparaît dans le nom d'utilisateur du bot — et sert à
    /// distinguer les instances dans les journaux.
    #[must_use]
    pub fn identifiant_bot(&self) -> &str {
        self.jeton_bot
            .split_once(':')
            .map_or(self.jeton_bot.as_str(), |(id, _)| id)
    }
}

/// Lit une variable obligatoire, en refusant la chaîne vide et les espaces seuls.
fn lire(nom: &'static str) -> Result<String, ErreurConfig> {
    match std::env::var(nom) {
        Ok(valeur) if !valeur.trim().is_empty() => Ok(valeur.trim().to_owned()),
        _ => Err(ErreurConfig::Absente(nom)),
    }
}

/// Lit une variable facultative, en retombant sur la valeur par défaut si elle est absente.
///
/// Délègue à [`lire`] : « ce qu'est une variable renseignée » n'a ainsi qu'une définition. Les
/// deux fonctions ont porté le même `match` recopié, ce qui aurait laissé la règle diverger en
/// silence le jour où l'une des deux aurait changé de politique sur les espaces.
fn lire_ou(nom: &'static str, defaut: &str) -> String {
    lire(nom).unwrap_or_else(|_| defaut.to_owned())
}

/// Vérifie qu'un jeton a la forme `<chiffres>:<35 caractères>` de `@BotFather`.
///
/// Le contrôle est une forme, pas une preuve : seul l'appel à `getMe` au démarrage prouve que
/// le jeton est valide. Il attrape la faute la plus fréquente — une valeur d'exemple laissée
/// en place, ou un jeton tronqué au copier-coller.
fn valider_jeton_bot(brut: &str) -> Result<(), ErreurConfig> {
    let invalide = |raison: &str| ErreurConfig::Invalide {
        variable: "TELEGRAM_BOT_TOKEN",
        raison: raison.to_owned(),
    };

    let Some((identifiant, secret)) = brut.split_once(':') else {
        return Err(invalide(
            "format attendu <identifiant>:<secret>, deux-points absent",
        ));
    };
    if identifiant.is_empty() || !identifiant.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalide(
            "la partie avant les deux-points doit être numérique",
        ));
    }
    if secret.len() != JETON_PARTIE_SECRETE {
        return Err(invalide(&format!(
            "la partie après les deux-points doit faire {JETON_PARTIE_SECRETE} caractères, elle en fait {}",
            secret.len()
        )));
    }
    if !secret
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalide(
            "la partie secrète contient un caractère hors de [A-Za-z0-9_-]",
        ));
    }
    Ok(())
}

/// Vérifie que le secret de webhook respecte le jeu de caractères de Telegram et notre plancher
/// d'entropie.
fn valider_secret(brut: &str) -> Result<(), ErreurConfig> {
    let invalide = |raison: String| ErreurConfig::Invalide {
        variable: "TELEGRAM_SECRET_WEBHOOK",
        raison,
    };

    if brut.len() < SECRET_LONGUEUR_MIN {
        return Err(invalide(format!(
            "au moins {SECRET_LONGUEUR_MIN} caractères exigés, celui-ci en fait {}",
            brut.len()
        )));
    }
    if brut.len() > SECRET_LONGUEUR_MAX {
        return Err(invalide(format!(
            "Telegram accepte au plus {SECRET_LONGUEUR_MAX} caractères, celui-ci en fait {}",
            brut.len()
        )));
    }
    if !brut
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalide(
            "Telegram n'accepte que [A-Za-z0-9_-] dans ce secret".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Un jeton de la bonne forme, qui ne correspond à aucun bot réel.
    const JETON_EXEMPLE: &str = "123456789:AAExempleDeJetonQuiNeSertAAbsolumen";
    const SECRET_EXEMPLE: &str = "un-secret-de-quarante-huit-caracteres-exactement";

    #[test]
    fn un_jeton_bien_forme_passe_et_livre_son_identifiant() {
        println!("jeton d'essai : {JETON_EXEMPLE}");
        println!("  partie secrète : {} caractères", JETON_EXEMPLE.len() - 10);
        valider_jeton_bot(JETON_EXEMPLE).expect("ce jeton doit être accepté");
        let config = Config {
            jeton_bot: JETON_EXEMPLE.to_owned(),
            secret_webhook: SECRET_EXEMPLE.to_owned(),
            adresse_ecoute: ADRESSE_ECOUTE_DEFAUT.parse().expect("adresse par défaut"),
            api_telegram: API_TELEGRAM_DEFAUT.to_owned(),
        };
        println!("  identifiant extrait : {}", config.identifiant_bot());
        assert_eq!(config.identifiant_bot(), "123456789");
    }

    #[test]
    fn les_jetons_mal_formes_sont_refuses_avec_la_raison() {
        let cas = [
            ("changeme", "pas de deux-points"),
            (
                "abc:AAExempleDeJetonQuiNeSertAAbsolume",
                "identifiant non numérique",
            ),
            ("123456789:trop-court", "partie secrète trop courte"),
            (
                "123456789:AAExempleDeJetonQuiNeSertAAbsolume!",
                "caractère interdit",
            ),
        ];
        for (brut, attendu) in cas {
            let erreur = valider_jeton_bot(brut).expect_err("ce jeton doit être refusé");
            println!("{attendu:32} -> {erreur}");
            // Le message d'erreur ne doit jamais citer la valeur fautive : il part en journal.
            assert!(
                !erreur.to_string().contains(brut),
                "la valeur fautive apparaît dans le message d'erreur"
            );
        }
    }

    #[test]
    fn un_secret_trop_court_ou_mal_composé_est_refusé() {
        let cas = [
            ("court", "moins de 32 caractères"),
            (
                "un-secret-assez-long-mais-avec-un-espace ici",
                "espace interdit par Telegram",
            ),
        ];
        for (brut, attendu) in cas {
            let erreur = valider_secret(brut).expect_err("ce secret doit être refusé");
            println!("{attendu:32} -> {erreur}");
            assert!(!erreur.to_string().contains(brut));
        }
        valider_secret(SECRET_EXEMPLE).expect("celui-ci doit passer");
        println!(
            "{:32} -> accepté ({} caractères)",
            "secret conforme",
            SECRET_EXEMPLE.len()
        );
    }

    #[test]
    fn le_debug_de_la_config_ne_laisse_fuir_aucun_secret() {
        let config = Config {
            jeton_bot: JETON_EXEMPLE.to_owned(),
            secret_webhook: SECRET_EXEMPLE.to_owned(),
            adresse_ecoute: ADRESSE_ECOUTE_DEFAUT.parse().expect("adresse par défaut"),
            api_telegram: API_TELEGRAM_DEFAUT.to_owned(),
        };
        let rendu = format!("{config:?}");
        println!("Debug rendu :\n  {rendu}");

        // La partie secrète du jeton, et le secret de webhook entier, sont absents.
        let partie_secrete = JETON_EXEMPLE.split_once(':').expect("jeton bien formé").1;
        assert!(
            !rendu.contains(partie_secrete),
            "la partie secrète du jeton apparaît dans le Debug"
        );
        assert!(
            !rendu.contains(SECRET_EXEMPLE),
            "le secret de webhook apparaît dans le Debug"
        );
        // L'identifiant du bot, lui, reste visible : c'est ce qui rend le journal utile.
        assert!(
            rendu.contains("123456789"),
            "l'identifiant du bot doit rester lisible"
        );
    }
}
