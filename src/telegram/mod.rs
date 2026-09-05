//! Le canal Telegram : authentifier ce qui entre, faire partir ce qui sort.
//!
//! # Le jeton est dans l'URL
//!
//! L'API Bot ne s'authentifie pas par en-tête : le jeton est un segment du chemin,
//! `https://api.telegram.org/bot<JETON>/sendMessage`. Toute URL est donc un secret, et la
//! conséquence traverse ce module — **jamais d'URL dans un journal, une erreur, ou un `Debug`**.
//! Les erreurs de [`envoi`] ne portent que le nom de la méthode, et c'est délibéré : un
//! `tracing::error!(%url)` bien intentionné suffirait à publier le jeton dans les journaux du
//! conteneur, et un jeton publié se remplace en révoquant le bot.
//!
//! # Ce qui distingue Telegram de n'importe qui
//!
//! Le webhook est une adresse publique. La seule chose qui distingue un appel de Telegram d'un
//! appel forgé est l'en-tête `X-Telegram-Bot-Api-Secret-Token`, que Telegram renvoie tel qu'on
//! le lui a donné à `setWebhook`. Il n'y a pas de signature cryptographique ici, contrairement
//! à ce que fait Meta : ce secret partagé *est* toute l'authentification, ce qui explique le
//! plancher de longueur imposé par [`crate::config`].

pub mod envoi;
pub mod types;

use std::time::Duration;

use axum::http::HeaderMap;
use serde::Serialize;

use crate::config::Config;
use crate::error::{ApiError, ErrorCode};
use envoi::{Action, ErreurEnvoi, Identite, MessageEnvoye, Panne, Reponse};

/// En-tête par lequel Telegram présente le secret partagé.
const ENTETE_SECRET: &str = "x-telegram-bot-api-secret-token";

/// Délai au-delà duquel un appel à Telegram est abandonné.
///
/// Telegram répond en quelques dizaines de millisecondes ; au-delà de quinze secondes, la
/// requête est perdue et insister ne fait que retenir une tâche.
const DELAI_APPEL: Duration = Duration::from_secs(15);

/// Délai d'établissement de la connexion.
const DELAI_CONNEXION: Duration = Duration::from_secs(5);

/// Ce qui a empêché la construction du canal.
#[derive(Debug, thiserror::Error)]
pub enum ErreurCanal {
    /// Le client HTTP n'a pas pu être construit.
    #[error("client HTTP inconstructible : {0}")]
    Client(#[from] reqwest::Error),
}

/// Le canal Telegram d'une instance.
///
/// Ne dérive ni `Debug` ni `Clone`, et les deux omissions sont délibérées. `Debug` rendrait le
/// jeton imprimable par accident. `Clone` serait pire à retardement : le canal est toujours
/// partagé par [`std::sync::Arc`], et une dérivation `Clone` laisserait un jour quelqu'un
/// écrire `canal: Canal` dans un état cloné à chaque requête — deux allocations de secret par
/// message, sans qu'aucun compilateur ne proteste. Interdire la copie du secret vers les
/// journaux tout en autorisant la copie de la structure entière n'aurait pas de sens.
pub struct Canal {
    client: reqwest::Client,
    /// `<racine api>/bot<jeton>` — secret.
    racine: String,
    /// Le secret attendu dans l'en-tête du webhook — secret.
    secret: String,
}

impl Canal {
    /// Construit le canal à partir de la configuration validée.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurCanal::Client`] si le client HTTP ne peut être construit — en pratique,
    /// une pile TLS indisponible.
    pub fn new(config: &Config) -> Result<Self, ErreurCanal> {
        let client = reqwest::Client::builder()
            .timeout(DELAI_APPEL)
            .connect_timeout(DELAI_CONNEXION)
            .build()?;

        Ok(Self {
            client,
            racine: format!("{}/bot{}", config.api_telegram, config.jeton_bot),
            secret: config.secret_webhook.clone(),
        })
    }

    /// Vérifie que l'appel vient bien de Telegram.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErrorCode::WebhookSecretInvalide`] si l'en-tête est absent, illisible, ou ne
    /// correspond pas. Les trois cas partagent **le même code et le même message public** :
    /// distinguer « absent » de « erroné » offrirait à qui sonde l'adresse un moyen de savoir
    /// qu'il approche.
    pub fn authentifier(&self, entetes: &HeaderMap) -> Result<(), ApiError> {
        let presente = entetes.get(ENTETE_SECRET).ok_or_else(|| {
            ApiError::new(ErrorCode::WebhookSecretInvalide, "en-tête de secret absent")
        })?;

        if egal_temps_constant(presente.as_bytes(), self.secret.as_bytes()) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::WebhookSecretInvalide,
                "le secret présenté ne correspond pas",
            ))
        }
    }

    /// Demande à Telegram qui est ce bot.
    ///
    /// Appelé au démarrage : c'est la seule preuve que le jeton est valide, et il vaut mieux
    /// l'obtenir avant d'accepter du trafic qu'au premier message d'un utilisateur.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurEnvoi`] si l'appel échoue ou si Telegram refuse le jeton.
    pub async fn identite(&self) -> Result<Identite, ErreurEnvoi> {
        self.appeler::<Identite, ()>("getMe", None).await
    }

    /// Envoie un texte, en autant de messages que nécessaire.
    ///
    /// Le découpage est fait ici et non par l'appelant : voir [`envoi::decouper`]. Renvoie les
    /// identifiants des messages créés, dans l'ordre.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurEnvoi`] au **premier** morceau qui échoue. Les morceaux précédents sont
    /// déjà partis et ne sont pas rappelés : Telegram ne sait pas défaire un envoi, et laisser
    /// un début de phrase vaut mieux que de faire disparaître ce que l'utilisateur a déjà lu.
    pub async fn envoyer_texte(&self, chat_id: i64, texte: &str) -> Result<Vec<i64>, ErreurEnvoi> {
        let morceaux = envoi::decouper(texte, envoi::LIMITE_TEXTE);
        let mut envoyes = Vec::with_capacity(morceaux.len());

        for morceau in morceaux {
            let corps = CorpsMessage {
                chat_id,
                text: morceau,
            };
            let message: MessageEnvoye = self.appeler("sendMessage", Some(&corps)).await?;
            envoyes.push(message.message_id);
        }

        Ok(envoyes)
    }

    /// Affiche « est en train d'écrire… » dans la discussion.
    ///
    /// L'indication s'efface d'elle-même au bout de cinq secondes, ou dès qu'un message part.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurEnvoi`] si l'appel échoue. L'appelant a le droit de l'ignorer : une
    /// action non affichée n'empêche pas la réponse d'arriver.
    pub async fn action(&self, chat_id: i64, action: Action) -> Result<(), ErreurEnvoi> {
        let corps = CorpsAction { chat_id, action };
        // Telegram renvoie `result: true` ; la valeur n'apprend rien de plus que l'absence
        // d'erreur.
        let _: bool = self.appeler("sendChatAction", Some(&corps)).await?;
        Ok(())
    }

    /// Déclare l'adresse du webhook auprès de Telegram, avec le secret partagé.
    ///
    /// `drop_pending_updates` est laissé à `false` : les messages arrivés pendant un
    /// redéploiement doivent être traités, pas jetés.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurEnvoi`] si Telegram refuse l'adresse — typiquement une adresse non
    /// HTTPS, ou un certificat qu'il ne valide pas.
    pub async fn declarer_webhook(&self, url: &str) -> Result<(), ErreurEnvoi> {
        let corps = CorpsWebhook {
            url,
            secret_token: &self.secret,
            allowed_updates: &["message"],
            drop_pending_updates: false,
        };
        let _: bool = self.appeler("setWebhook", Some(&corps)).await?;
        Ok(())
    }

    /// Retire le webhook. Telegram cesse d'appeler jusqu'à une nouvelle déclaration.
    ///
    /// # Errors
    ///
    /// Renvoie [`ErreurEnvoi`] si l'appel échoue.
    pub async fn retirer_webhook(&self) -> Result<(), ErreurEnvoi> {
        let _: bool = self.appeler::<bool, ()>("deleteWebhook", None).await?;
        Ok(())
    }

    /// Le corps commun de tout appel à l'API Bot.
    ///
    /// Aucune des erreurs construites ici ne porte l'URL — seulement `methode`. C'est la règle
    /// énoncée en tête de module, appliquée en un seul endroit pour qu'elle ne puisse pas
    /// être oubliée ailleurs.
    async fn appeler<R, C>(
        &self,
        methode: &'static str,
        corps: Option<&C>,
    ) -> Result<R, ErreurEnvoi>
    where
        R: serde::de::DeserializeOwned,
        C: Serialize + ?Sized,
    {
        let url = format!("{}/{methode}", self.racine);
        let requete = self.client.post(&url);
        let requete = match corps {
            Some(c) => requete.json(c),
            None => requete,
        };

        let reponse = requete.send().await.map_err(|source| ErreurEnvoi::Reseau {
            methode,
            // `Panne::classer` prend la référence et n'en retient que la nature : l'URL,
            // qui porte le jeton, meurt avec `source` à la fin de cette closure.
            panne: Panne::classer(&source),
        })?;

        // Le statut HTTP n'est pas consulté : Telegram décrit ses refus dans le corps, avec un
        // `error_code` plus précis que le statut. Lire le corps dans tous les cas donne une
        // erreur exploitable là où un `error_for_status` ne donnerait qu'un nombre.
        let enveloppe: Reponse<R> =
            reponse
                .json()
                .await
                .map_err(|source| ErreurEnvoi::Illisible {
                    methode,
                    panne: Panne::classer(&source),
                })?;

        enveloppe.deplier(methode)
    }
}

/// Le corps d'un `sendMessage`.
#[derive(Debug, Serialize)]
struct CorpsMessage<'a> {
    chat_id: i64,
    text: &'a str,
}

/// Le corps d'un `sendChatAction`.
#[derive(Debug, Serialize)]
struct CorpsAction {
    chat_id: i64,
    action: Action,
}

/// Le corps d'un `setWebhook`.
#[derive(Debug, Serialize)]
struct CorpsWebhook<'a> {
    url: &'a str,
    secret_token: &'a str,
    /// Restreindre ce que Telegram envoie réduit d'autant la surface à valider.
    allowed_updates: &'a [&'a str],
    drop_pending_updates: bool,
}

/// Compare deux suites d'octets sans révéler par sa durée où elles divergent.
///
/// # Ce que cela protège, et ce que cela ne protège pas
///
/// Une comparaison ordinaire s'arrête au premier octet différent. Un attaquant qui mesure des
/// milliers de réponses peut alors reconstituer le secret octet par octet. La boucle ici
/// parcourt toujours toute la longueur.
///
/// Ce que la fonction ne masque pas, c'est la **longueur** du secret présenté : les tailles
/// sont comparées d'abord. C'est assumé — la longueur de notre secret est une constante de
/// déploiement, pas une information que sa découverte rendrait exploitable.
fn egal_temps_constant(presente: &[u8], attendu: &[u8]) -> bool {
    if presente.len() != attendu.len() {
        return false;
    }
    let mut ecart = 0_u8;
    for (a, b) in presente.iter().zip(attendu) {
        ecart |= a ^ b;
    }
    ecart == 0
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "un-secret-de-quarante-huit-caracteres-exactement";

    fn canal_de_test() -> Canal {
        let config = Config {
            jeton_bot: "123456789:AAExempleDeJetonQuiNeSertAAbsolumen".to_owned(),
            secret_webhook: SECRET.to_owned(),
            adresse_ecoute: "127.0.0.1:0".parse().expect("adresse littérale"),
            api_telegram: "https://api.telegram.org".to_owned(),
        };
        Canal::new(&config).expect("le client doit se construire")
    }

    fn entetes_avec(secret: &str) -> HeaderMap {
        let mut entetes = HeaderMap::new();
        entetes.insert(
            ENTETE_SECRET,
            HeaderValue::from_str(secret).expect("valeur d'en-tête ASCII"),
        );
        entetes
    }

    #[test]
    fn le_bon_secret_passe_et_tous_les_autres_echouent_sur_le_meme_code() {
        let canal = canal_de_test();

        canal
            .authentifier(&entetes_avec(SECRET))
            .expect("le secret exact doit être accepté");
        println!("secret exact                     -> accepté");

        let refus: Vec<(&str, HeaderMap)> = vec![
            ("en-tête absent", HeaderMap::new()),
            ("secret vide", entetes_avec("")),
            ("un caractère de trop", entetes_avec(&format!("{SECRET}x"))),
            (
                "un caractère de moins",
                entetes_avec(&SECRET[..SECRET.len() - 1]),
            ),
            (
                "dernier caractère changé",
                entetes_avec(&format!("{}X", &SECRET[..SECRET.len() - 1])),
            ),
            (
                "premier caractère changé",
                entetes_avec(&format!("X{}", &SECRET[1..])),
            ),
        ];

        for (nom, entetes) in refus {
            let erreur = canal
                .authentifier(&entetes)
                .expect_err("ce cas doit être refusé");
            println!(
                "{nom:32} -> refusé code={} statut={} message={:?}",
                erreur.code().code(),
                erreur.code().statut(),
                erreur.code().message_public()
            );
            assert_eq!(
                erreur.code(),
                ErrorCode::WebhookSecretInvalide,
                "« {nom} » doit être indiscernable des autres refus"
            );
        }
    }

    #[test]
    fn la_comparaison_ne_confond_pas_prefixe_et_egalite() {
        let cas = [
            (&b"abc"[..], &b"abc"[..], true),
            (b"abc", b"abcd", false),
            (b"abcd", b"abc", false),
            (b"abc", b"abd", false),
            (b"", b"", true),
        ];
        for (a, b, attendu) in cas {
            let obtenu = egal_temps_constant(a, b);
            println!(
                "{:>6} vs {:<6} -> {obtenu}",
                String::from_utf8_lossy(a),
                String::from_utf8_lossy(b)
            );
            assert_eq!(obtenu, attendu);
        }
    }

    #[test]
    fn le_canal_ne_peut_pas_imprimer_son_jeton() {
        // `Canal` ne dérive pas Debug : cette assertion est tenue par le compilateur, pas par
        // un test. Ce qui suit vérifie l'autre moitié — que les erreurs d'envoi, elles, ne
        // portent que le nom de la méthode.
        let erreur = ErreurEnvoi::Api {
            methode: "sendMessage",
            code: 401,
            description: "Unauthorized".to_owned(),
            retry_after: None,
        };
        let rendu = format!("{erreur} | {erreur:?}");
        println!("erreur rendue : {rendu}");
        assert!(
            !rendu.contains("AAExemple"),
            "la partie secrète du jeton a fuité"
        );
        assert!(!rendu.contains("/bot"), "une URL de l'API a fuité");
        assert!(
            rendu.contains("sendMessage"),
            "la méthode doit rester identifiable"
        );
    }
}
