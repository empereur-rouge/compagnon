//! Les formes que Telegram envoie, et celle qu'on en retient.
//!
//! # Ce qui est modélisé, et ce qui ne l'est pas
//!
//! Une `Update` Telegram peut porter une trentaine de champs. Seuls ceux dont le service se
//! sert sont déclarés : `serde` ignore silencieusement le reste, et un champ non déclaré est
//! un champ dont personne n'a à se demander ce qu'il vaut. Les phases suivantes en ajouteront
//! — `voice` en phase 4, `callback_query` quand il y aura des boutons.
//!
//! # Pourquoi une structure d'extraction séparée
//!
//! [`Update`] est la forme de Telegram : tout y est facultatif, parce que Telegram décide.
//! [`Recu`] est la forme du service : tout y est certain, parce que l'extraction a déjà
//! tranché. Le reste du code ne manipule que [`Recu`] et n'a donc jamais à se demander si un
//! message a bien un auteur.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use tracing::Level;

use super::envoi::longueur_utf16;

/// Longueur maximale d'un texte entrant, en unités UTF-16.
///
/// C'est la limite de Telegram lui-même, revérifiée ici : le service peut être branché sur un
/// serveur Bot API local, qui ne l'impose pas nécessairement, et un message démesuré ne doit
/// pas traverser la suite du traitement.
///
/// Distincte de [`super::envoi::LIMITE_TEXTE`] malgré une valeur identique : ce sont deux
/// limites de sens opposé — ce qu'on accepte de lire, et ce qu'on peut faire partir. Telegram
/// pourrait faire bouger l'une sans l'autre, et les confondre reviendrait à laisser une
/// évolution de l'API sortante changer ce qu'on accepte en entrée.
pub const TAILLE_MAX_TEXTE_ENTRANT: usize = 4096;

/// Une mise à jour, telle que Telegram la poste sur le webhook.
#[derive(Debug, Deserialize)]
pub struct Update {
    /// Identifiant croissant de la mise à jour. Sert au dédoublonnage à partir de la phase 1.
    pub update_id: i64,
    /// Un message neuf.
    #[serde(default)]
    pub message: Option<Message>,
    /// Un message que l'utilisateur a modifié après coup.
    ///
    /// Déclaré pour être **explicitement ignoré** : sans ce champ, une modification tomberait
    /// dans la même absence qu'une mise à jour inconnue, et on ne pourrait pas les distinguer
    /// dans les journaux.
    ///
    /// Le type dit l'intention et l'applique. Un `Option<Message>` aurait fait construire à
    /// `serde` un message entier — allocations du texte et du prénom comprises — pour le jeter
    /// à la ligne suivante ; `IgnoredAny` consomme la valeur sans rien bâtir.
    #[serde(default)]
    pub edited_message: Option<IgnoredAny>,
}

/// Un message Telegram.
#[derive(Debug, Deserialize)]
pub struct Message {
    /// Identifiant du message dans sa discussion.
    pub message_id: i64,
    /// L'auteur. Absent pour les messages postés au nom d'un canal.
    #[serde(default)]
    pub from: Option<Utilisateur>,
    /// La discussion où le message a été posté.
    pub chat: Discussion,
    /// Date d'envoi, en secondes depuis l'époque Unix.
    pub date: i64,
    /// Le texte, absent si le message est une photo, un autocollant, une note vocale…
    #[serde(default)]
    pub text: Option<String>,
}

/// L'auteur d'un message.
#[derive(Debug, Deserialize)]
pub struct Utilisateur {
    /// Identifiant Telegram, stable et unique.
    pub id: i64,
    /// Vrai si l'auteur est lui-même un bot.
    pub is_bot: bool,
    /// Prénom déclaré. Toujours présent chez Telegram.
    pub first_name: String,
    /// Nom d'utilisateur, sans l'arobase. Facultatif chez Telegram.
    #[serde(default)]
    pub username: Option<String>,
}

/// La discussion où un message a été posté.
#[derive(Debug, Deserialize)]
pub struct Discussion {
    /// Identifiant de la discussion. En privé, il est égal à celui de l'utilisateur.
    pub id: i64,
    /// `private`, `group`, `supergroup` ou `channel`.
    #[serde(rename = "type")]
    pub genre: String,
}

/// Pourquoi une mise à jour n'a pas donné lieu à un [`Recu`].
///
/// Un énuméré et non un simple `None` : ces raisons partent en journal agrégé, et savoir qu'on
/// écarte trois cents messages de groupe par jour n'est pas la même information que savoir
/// qu'on en écarte trois cents illisibles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecart {
    /// La mise à jour ne porte aucun message (modification, accusé, type non géré).
    SansMessage,
    /// Le message a été modifié après envoi : le service ne réécrit pas le passé.
    Modification,
    /// La discussion n'est pas un tête-à-tête.
    HorsPrive,
    /// L'auteur est un bot.
    AuteurBot,
    /// Le message n'a pas d'auteur identifiable.
    SansAuteur,
    /// Le message ne porte pas de texte.
    SansTexte,
    /// Le texte est vide, ou n'est fait que d'espaces.
    TexteVide,
    /// Le texte dépasse [`TAILLE_MAX_TEXTE_ENTRANT`].
    TexteDemesure,
}

impl Ecart {
    /// Le niveau auquel cet écart mérite d'être journalisé.
    ///
    /// Porté par l'énuméré, comme [`crate::error::ErrorCode::niveau`], pour que le `match` de
    /// [`crate::webhook`] reste exhaustif : un bras attrape-tout ferait tomber en silence toute
    /// variante ajoutée par une phase suivante dans le niveau le plus bas.
    #[must_use]
    pub const fn niveau(self) -> Level {
        match self {
            // Telegram n'est pas censé livrer autre chose que ce que `allowed_updates`
            // demande : en voir mérite un regard, sans mode debug.
            Self::SansMessage => Level::INFO,
            // Un texte au-delà du plafond de Telegram ne peut venir que d'un serveur Bot API
            // local mal réglé, ou d'un appel forgé.
            Self::TexteDemesure => Level::WARN,
            // Fonctionnement normal : un autocollant, un groupe, un message corrigé.
            Self::Modification
            | Self::HorsPrive
            | Self::AuteurBot
            | Self::SansAuteur
            | Self::SansTexte
            | Self::TexteVide => Level::DEBUG,
        }
    }

    /// Libellé court, pour les journaux.
    #[must_use]
    pub const fn libelle(self) -> &'static str {
        match self {
            Self::SansMessage => "sans message",
            Self::Modification => "message modifié",
            Self::HorsPrive => "hors discussion privée",
            Self::AuteurBot => "auteur bot",
            Self::SansAuteur => "sans auteur",
            Self::SansTexte => "sans texte",
            Self::TexteVide => "texte vide",
            Self::TexteDemesure => "texte démesuré",
        }
    }
}

/// Un message entrant dont tout est certain.
///
/// Sérialisable parce qu'il transite désormais par la base : la file porte la charge utile en
/// `jsonb`, et c'est ce qui lui permet de survivre à l'arrêt du processus.
#[derive(Debug, Serialize, Deserialize)]
pub struct Recu {
    /// Où répondre.
    pub chat_id: i64,
    /// Qui a écrit. Clé de l'utilisateur à partir de la phase 1.
    pub utilisateur_id: i64,
    /// Le message auquel on répond, pour `reply_to_message_id` si besoin.
    pub message_id: i64,
    /// Prénom déclaré, utile au personnage dès le premier mot.
    pub prenom: String,
    /// Le texte, découpé de ses espaces de bord.
    pub texte: String,
    /// Quand Telegram l'a reçu, en secondes depuis l'époque Unix.
    pub recu_le: i64,
}

impl Update {
    /// Retient ce qui mérite une réponse, ou dit pourquoi rien ne la mérite.
    ///
    /// # Ce qui est écarté, et pourquoi
    ///
    /// - **les discussions de groupe** — un compagnon est un tête-à-tête ; répondre dans un
    ///   groupe exposerait à tous une conversation intime et changerait la nature du produit ;
    /// - **les autres bots** — deux bots qui se répondent produisent une boucle infinie
    ///   facturée à chaque tour ;
    /// - **les messages modifiés** — la mémoire du personnage est un récit, et un récit ne se
    ///   réécrit pas rétroactivement ;
    /// - **tout ce qui n'est pas du texte** — la phase 0 ne sait pas faire ; ces messages
    ///   seront pris en charge phase 4 (voix) et non silencieusement perdus.
    ///
    /// # Errors
    ///
    /// Renvoie l'[`Ecart`] qui a motivé le rejet.
    pub fn extraire(self) -> Result<Recu, Ecart> {
        if self.edited_message.is_some() {
            return Err(Ecart::Modification);
        }
        let message = self.message.ok_or(Ecart::SansMessage)?;

        if message.chat.genre != "private" {
            return Err(Ecart::HorsPrive);
        }
        let auteur = message.from.ok_or(Ecart::SansAuteur)?;
        if auteur.is_bot {
            return Err(Ecart::AuteurBot);
        }

        let texte = message.text.ok_or(Ecart::SansTexte)?;
        let texte = texte.trim();
        if texte.is_empty() {
            return Err(Ecart::TexteVide);
        }
        if longueur_utf16(texte) > TAILLE_MAX_TEXTE_ENTRANT {
            return Err(Ecart::TexteDemesure);
        }

        Ok(Recu {
            chat_id: message.chat.id,
            utilisateur_id: auteur.id,
            message_id: message.message_id,
            prenom: auteur.first_name,
            texte: texte.to_owned(),
            recu_le: message.date,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Une mise à jour privée ordinaire, telle que Telegram l'envoie réellement.
    fn update_privee(texte: &str) -> Update {
        let brut = serde_json::json!({
            "update_id": 900_001,
            "message": {
                "message_id": 17,
                "from": {
                    "id": 42, "is_bot": false, "first_name": "Erwan",
                    "username": "erwan", "language_code": "fr"
                },
                "chat": { "id": 42, "first_name": "Erwan", "type": "private" },
                "date": 1_760_000_000_i64,
                "text": texte
            }
        });
        serde_json::from_value(brut).expect("cette forme est celle de Telegram")
    }

    #[test]
    fn un_message_prive_ordinaire_est_retenu_en_entier() {
        let recu = update_privee("  salut, tu fais quoi ?  ")
            .extraire()
            .expect("ce message doit être retenu");
        println!("reçu extrait : {recu:#?}");
        assert_eq!(recu.chat_id, 42);
        assert_eq!(recu.utilisateur_id, 42);
        assert_eq!(recu.message_id, 17);
        assert_eq!(recu.prenom, "Erwan");
        // Les espaces de bord sont retirés, le contenu ne l'est pas.
        assert_eq!(recu.texte, "salut, tu fais quoi ?");
        assert_eq!(recu.recu_le, 1_760_000_000);
    }

    #[test]
    fn les_champs_inconnus_de_telegram_ne_font_pas_echouer_la_lecture() {
        // Telegram ajoute des champs sans prévenir ; une mise à jour enrichie doit passer.
        let brut = serde_json::json!({
            "update_id": 900_002,
            "champ_invente_par_telegram_demain": { "quoi": "que ce soit" },
            "message": {
                "message_id": 18,
                "from": { "id": 42, "is_bot": false, "first_name": "Erwan" },
                "chat": { "id": 42, "type": "private" },
                "date": 1_760_000_001_i64,
                "text": "et ça ?",
                "un_autre_champ_neuf": [1, 2, 3]
            }
        });
        let update: Update = serde_json::from_value(brut).expect("les champs neufs sont ignorés");
        let recu = update.extraire().expect("le message reste exploitable");
        println!(
            "texte extrait malgré les champs inconnus : {:?}",
            recu.texte
        );
        assert_eq!(recu.texte, "et ça ?");
    }

    #[test]
    fn chaque_ecart_porte_son_libelle_et_son_niveau() {
        // L'exhaustivité est tenue par le compilateur sur `libelle` et `niveau` ; ce test
        // rend le tableau lisible et vérifie qu'aucun libellé n'est vide.
        for ecart in [
            Ecart::SansMessage,
            Ecart::Modification,
            Ecart::HorsPrive,
            Ecart::AuteurBot,
            Ecart::SansAuteur,
            Ecart::SansTexte,
            Ecart::TexteVide,
            Ecart::TexteDemesure,
        ] {
            println!(
                "{:<16} niveau={:<5} « {} »",
                format!("{ecart:?}"),
                ecart.niveau(),
                ecart.libelle()
            );
            assert!(!ecart.libelle().is_empty());
        }
    }

    #[test]
    fn chaque_motif_d_ecart_est_reconnu_pour_ce_qu_il_est() {
        let cas: Vec<(&str, serde_json::Value, Ecart)> = vec![
            (
                "discussion de groupe",
                serde_json::json!({"update_id": 1, "message": {
                    "message_id": 1, "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
                    "chat": {"id": -100, "type": "supergroup"}, "date": 1, "text": "coucou"}}),
                Ecart::HorsPrive,
            ),
            (
                "auteur bot",
                serde_json::json!({"update_id": 2, "message": {
                    "message_id": 2, "from": {"id": 7, "is_bot": true, "first_name": "AutreBot"},
                    "chat": {"id": 7, "type": "private"}, "date": 1, "text": "coucou"}}),
                Ecart::AuteurBot,
            ),
            (
                "message modifié",
                serde_json::json!({"update_id": 3, "edited_message": {
                    "message_id": 3, "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
                    "chat": {"id": 42, "type": "private"}, "date": 1, "text": "corrigé"}}),
                Ecart::Modification,
            ),
            (
                "photo sans légende",
                serde_json::json!({"update_id": 4, "message": {
                    "message_id": 4, "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
                    "chat": {"id": 42, "type": "private"}, "date": 1}}),
                Ecart::SansTexte,
            ),
            (
                "texte fait d'espaces",
                serde_json::json!({"update_id": 5, "message": {
                    "message_id": 5, "from": {"id": 42, "is_bot": false, "first_name": "Erwan"},
                    "chat": {"id": 42, "type": "private"}, "date": 1, "text": "   \n  "}}),
                Ecart::TexteVide,
            ),
            (
                "accusé de lecture, sans message",
                serde_json::json!({"update_id": 6}),
                Ecart::SansMessage,
            ),
        ];

        for (nom, brut, attendu) in cas {
            let update: Update = serde_json::from_value(brut).expect("forme lisible");
            let obtenu = update.extraire().expect_err("ce cas doit être écarté");
            println!("{nom:32} -> écarté : {}", obtenu.libelle());
            assert_eq!(obtenu, attendu, "mauvais motif d'écart pour « {nom} »");
        }
    }

    #[test]
    fn un_texte_demesure_est_ecarte_avant_le_reste_du_traitement() {
        // Telegram plafonne à 4096 ; un serveur Bot API local, non.
        let enorme = "é".repeat(TAILLE_MAX_TEXTE_ENTRANT + 1);
        println!(
            "texte de {} caractères, {} unités UTF-16, {} octets",
            enorme.chars().count(),
            longueur_utf16(&enorme),
            enorme.len()
        );
        let ecart = update_privee(&enorme)
            .extraire()
            .expect_err("ce texte doit être écarté");
        println!("écarté : {}", ecart.libelle());
        assert_eq!(ecart, Ecart::TexteDemesure);

        // Et le cas limite exact passe, lui.
        let pile = "é".repeat(TAILLE_MAX_TEXTE_ENTRANT);
        let recu = update_privee(&pile)
            .extraire()
            .expect("4096 pile doit passer");
        println!("4096 unités UTF-16 : retenu ({} octets)", recu.texte.len());
    }
}
