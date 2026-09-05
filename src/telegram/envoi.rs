//! Ce que le service envoie à Telegram, et ce qui peut l'en empêcher.
//!
//! # Le découpage n'est pas un détail
//!
//! Telegram refuse tout message de plus de 4096 unités UTF-16. Un personnage qui raconte
//! quelque chose dépasse cette limite régulièrement, et l'API répond alors `400` en jetant le
//! message entier — l'utilisateur ne voit rien du tout. Le découpage vit donc **sous** l'envoi,
//! pas au-dessus : aucun appelant ne peut oublier de s'en servir.
//!
//! Le point de coupe est choisi, pas subi : on recule jusqu'au dernier saut de ligne, sinon
//! jusqu'à la dernière espace, tant que cela ne sacrifie pas plus d'un quart du morceau.
//! Couper au milieu d'un mot se voit ; couper à la fin d'un paragraphe ne se voit pas.
//!
//! # Pourquoi aucun `parse_mode`
//!
//! Le texte part brut. Demander `MarkdownV2` obligerait à échapper dix-huit caractères dans un
//! texte que le modèle produit librement, et un seul échappement manqué fait rejeter le
//! message entier par Telegram. Le formatage viendra quand un besoin réel le justifiera, avec
//! son échappement et ses tests.

use serde::{Deserialize, Serialize};

/// Longueur maximale d'un message sortant, en unités UTF-16. Limite de Telegram.
pub const LIMITE_TEXTE: usize = 4096;

/// Proportion du morceau qu'on accepte de sacrifier pour couper proprement.
///
/// Reculer jusqu'à la dernière espace est souhaitable ; reculer de 3000 caractères pour la
/// trouver ne l'est pas — cela produirait un message ridiculement court suivi d'un autre
/// démesuré. Au-delà de ce recul, on coupe net.
const RECUL_MAX: usize = 4;

/// Ce que le bot fait mine de faire pendant qu'il prépare sa réponse.
///
/// Sans cela, la réponse surgit d'un coup et le personnage se dénonce comme une machine. C'est
/// le seul artifice de la phase 0 qui relève de l'illusion plutôt que du transport, et il est
/// ici parce que la latence jouée se règle au même endroit que l'envoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// « est en train d'écrire… »
    Typing,
    /// « envoie une photo… » — phase 3.
    UploadPhoto,
    /// « enregistre un message vocal… » — phase 4.
    RecordVoice,
}

/// Ce qui a empêché un envoi d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurEnvoi {
    /// La requête n'a pas abouti : réseau, TLS, délai dépassé.
    ///
    /// L'URL n'apparaît jamais dans ce message : elle contient le jeton du bot.
    #[error("appel « {methode} » impossible : {source}")]
    Reseau {
        /// La méthode Bot API visée, seul élément d'identification qu'on journalise.
        methode: &'static str,
        /// La cause côté client.
        source: reqwest::Error,
    },

    /// Telegram a répondu, et a refusé.
    #[error("Telegram a refusé « {methode} » : {code} {description}")]
    Api {
        /// La méthode Bot API visée.
        methode: &'static str,
        /// Le code d'erreur de Telegram (400, 401, 403, 429…).
        code: i32,
        /// La description renvoyée par Telegram.
        description: String,
        /// Le délai avant nouvelle tentative, quand Telegram limite le débit (429).
        retry_after: Option<u64>,
    },

    /// Telegram a répondu quelque chose d'inattendu.
    #[error("réponse de « {methode} » illisible : {source}")]
    Illisible {
        /// La méthode Bot API visée.
        methode: &'static str,
        /// La cause de la lecture manquée.
        source: reqwest::Error,
    },
}

impl ErreurEnvoi {
    /// Vrai si réessayer plus tard a une chance d'aboutir.
    ///
    /// Distinguer ces deux familles n'est pas cosmétique : réessayer une erreur définitive
    /// (`403 bot was blocked by the user`) refait le même appel jusqu'à épuisement du quota,
    /// et abandonner une erreur transitoire perd le message d'une personne qui l'attend.
    #[must_use]
    pub const fn merite_une_reprise(&self) -> bool {
        match self {
            // Réseau, TLS, délai : l'appel n'a pas abouti, l'état distant est inchangé.
            Self::Reseau { .. } => true,
            // Une réponse illisible vient d'un intermédiaire (portail captif, proxy) plutôt
            // que de Telegram : c'est transitoire.
            Self::Illisible { .. } => true,
            Self::Api { code, .. } => match *code {
                // Débit dépassé : c'est précisément le cas qui demande une reprise différée.
                429 => true,
                // 5xx côté Telegram.
                500..=599 => true,
                // 400 (message mal formé), 401 (jeton révoqué), 403 (bot bloqué) : refaire le
                // même appel refera la même erreur.
                _ => false,
            },
        }
    }

    /// Le délai que Telegram demande d'observer, s'il en a donné un.
    #[must_use]
    pub const fn attendre(&self) -> Option<u64> {
        match self {
            Self::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// L'enveloppe que Telegram met autour de toute réponse.
#[derive(Debug, Deserialize)]
pub(super) struct Reponse<T> {
    pub(super) ok: bool,
    pub(super) result: Option<T>,
    pub(super) error_code: Option<i32>,
    pub(super) description: Option<String>,
    pub(super) parameters: Option<Parametres>,
}

/// Les précisions que Telegram joint à certains refus.
#[derive(Debug, Deserialize)]
pub(super) struct Parametres {
    /// Secondes à attendre avant de réessayer, sur un `429`.
    #[serde(default)]
    pub(super) retry_after: Option<u64>,
}

impl<T> Reponse<T> {
    /// Déplie l'enveloppe, ou construit l'erreur correspondante.
    pub(super) fn deplier(self, methode: &'static str) -> Result<T, ErreurEnvoi> {
        match (self.ok, self.result) {
            (true, Some(resultat)) => Ok(resultat),
            // `ok: true` sans résultat ne devrait pas exister ; le traiter comme un refus
            // plutôt que paniquer.
            _ => Err(ErreurEnvoi::Api {
                methode,
                code: self.error_code.unwrap_or(0),
                description: self
                    .description
                    .unwrap_or_else(|| "sans description".to_owned()),
                retry_after: self.parameters.and_then(|p| p.retry_after),
            }),
        }
    }
}

/// Qui est le bot, d'après `getMe`.
#[derive(Debug, Clone, Deserialize)]
pub struct Identite {
    /// Identifiant numérique du bot.
    pub id: i64,
    /// Nom affiché.
    pub first_name: String,
    /// Nom d'utilisateur, sans l'arobase.
    pub username: Option<String>,
}

/// Le message renvoyé par Telegram après un envoi réussi.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEnvoye {
    /// Identifiant du message créé.
    pub message_id: i64,
}

/// Longueur d'un texte en unités UTF-16 — l'unité que Telegram compte.
///
/// Compter les caractères Rust donnerait un résultat faux : un emoji hors du plan de base vaut
/// une `char` et deux unités UTF-16. Un message de 4096 emojis serait accepté ici et refusé
/// par Telegram.
#[must_use]
pub fn longueur_utf16(texte: &str) -> usize {
    texte.chars().map(char::len_utf16).sum()
}

/// Découpe un texte en morceaux qu'un `sendMessage` accepte.
///
/// Renvoie toujours au moins un morceau pour un texte non vide, et jamais de morceau vide.
/// Les morceaux, concaténés avec une espace, redonnent le texte à la ponctuation d'espacement
/// près — la coupe consomme le séparateur sur lequel elle tombe.
///
/// # Examples
///
/// ```
/// use compagnon::telegram::envoi::{decouper, LIMITE_TEXTE};
///
/// let court = decouper("bonjour", LIMITE_TEXTE);
/// assert_eq!(court, vec!["bonjour"]);
///
/// // Sur une limite serrée, la coupe tombe sur l'espace, pas au milieu d'un mot.
/// assert_eq!(decouper("alpha beta gamma", 11), vec!["alpha beta", "gamma"]);
/// ```
#[must_use]
pub fn decouper(texte: &str, limite: usize) -> Vec<&str> {
    let mut morceaux = Vec::new();
    let mut reste = texte.trim();

    while !reste.is_empty() {
        if longueur_utf16(reste) <= limite {
            morceaux.push(reste);
            break;
        }

        // Le plus long préfixe qui tienne dans la limite, sur une frontière de caractère.
        let mut fin = 0;
        let mut unites = 0;
        for (index, caractere) in reste.char_indices() {
            let poids = caractere.len_utf16();
            if unites + poids > limite {
                break;
            }
            unites += poids;
            fin = index + caractere.len_utf8();
        }

        // `fin == 0` signifierait qu'un seul caractère dépasse la limite : impossible tant que
        // la limite vaut au moins 2, mais s'en remettre à cette hypothèse ferait une boucle
        // infinie le jour où quelqu'un appelle avec `limite = 1`.
        if fin == 0 {
            fin = reste
                .char_indices()
                .nth(1)
                .map_or(reste.len(), |(index, _)| index);
        }

        let coupe = point_de_coupe(&reste[..fin]);
        let morceau = reste[..coupe].trim_end();
        if !morceau.is_empty() {
            morceaux.push(morceau);
        }
        reste = reste[coupe..].trim_start();
    }

    morceaux
}

/// Choisit où couper dans un préfixe qui tient déjà dans la limite.
///
/// Recule jusqu'au dernier saut de ligne, sinon jusqu'à la dernière espace, tant que le recul
/// ne dépasse pas [`RECUL_MAX`] du préfixe. Sinon coupe au bout.
fn point_de_coupe(prefixe: &str) -> usize {
    let plancher = prefixe.len() - prefixe.len() / RECUL_MAX;
    for separateur in ['\n', ' '] {
        if let Some(index) = prefixe.rfind(separateur)
            && index >= plancher
        {
            return index + separateur.len_utf8();
        }
    }
    prefixe.len()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn un_texte_court_reste_en_un_seul_morceau() {
        let morceaux = decouper("salut, ça va ?", LIMITE_TEXTE);
        println!("{} morceau(x) : {morceaux:?}", morceaux.len());
        assert_eq!(morceaux, vec!["salut, ça va ?"]);
    }

    #[test]
    fn la_coupe_prefere_un_saut_de_ligne_puis_une_espace() {
        let texte = "premier paragraphe\ndeuxieme paragraphe qui suit";
        let morceaux = decouper(texte, 22);
        for (rang, morceau) in morceaux.iter().enumerate() {
            println!(
                "morceau {rang} ({} unités UTF-16) : {morceau:?}",
                longueur_utf16(morceau)
            );
        }
        assert_eq!(
            morceaux[0], "premier paragraphe",
            "la coupe doit tomber sur le \\n"
        );

        let sans_ligne = "alpha beta gamma delta epsilon";
        let morceaux = decouper(sans_ligne, 18);
        for (rang, morceau) in morceaux.iter().enumerate() {
            println!(
                "sans \\n, morceau {rang} ({} unités) : {morceau:?}",
                longueur_utf16(morceau)
            );
        }
        assert_eq!(
            morceaux[0], "alpha beta gamma",
            "la coupe doit tomber sur l'espace, pas au milieu d'un mot"
        );
    }

    #[test]
    fn aucun_morceau_ne_depasse_la_limite_et_aucun_n_est_vide() {
        // Un texte réaliste de personnage : des paragraphes, des accents, des emojis.
        let paragraphe = "Elle repose sa tasse et te regarde un instant sans rien dire 🌙 ";
        let texte = paragraphe.repeat(300);
        println!(
            "texte d'entrée : {} unités UTF-16, {} octets",
            longueur_utf16(&texte),
            texte.len()
        );

        let morceaux = decouper(&texte, LIMITE_TEXTE);
        println!("découpé en {} morceaux :", morceaux.len());
        for (rang, morceau) in morceaux.iter().enumerate() {
            let unites = longueur_utf16(morceau);
            println!(
                "  {rang:>2} : {unites:>5} unités | début {:?} | fin {:?}",
                &morceau.chars().take(28).collect::<String>(),
                &morceau
                    .chars()
                    .rev()
                    .take(18)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            );
            assert!(unites <= LIMITE_TEXTE, "morceau {rang} dépasse la limite");
            assert!(!morceau.trim().is_empty(), "morceau {rang} est vide");
        }
        assert!(morceaux.len() > 1, "ce texte doit être découpé");
    }

    #[test]
    fn un_emoji_compte_pour_deux_unites_utf16() {
        // 🌙 est hors du plan de base : une char Rust, deux unités UTF-16. Compter les chars
        // laisserait passer un message que Telegram refuserait.
        let emoji = "🌙";
        println!(
            "{emoji} : {} char, {} unités UTF-16, {} octets",
            emoji.chars().count(),
            longueur_utf16(emoji),
            emoji.len()
        );
        assert_eq!(emoji.chars().count(), 1);
        assert_eq!(longueur_utf16(emoji), 2);

        // 2049 emojis = 4098 unités : au-delà de la limite, donc découpé.
        let texte = emoji.repeat(2049);
        let morceaux = decouper(&texte, LIMITE_TEXTE);
        println!(
            "2049 emojis ({} unités) -> {} morceaux de {:?} unités",
            longueur_utf16(&texte),
            morceaux.len(),
            morceaux
                .iter()
                .map(|m| longueur_utf16(m))
                .collect::<Vec<_>>()
        );
        assert_eq!(morceaux.len(), 2);
        for morceau in &morceaux {
            assert!(longueur_utf16(morceau) <= LIMITE_TEXTE);
        }
    }

    #[test]
    fn un_mot_plus_long_que_la_limite_est_coupe_net_sans_boucler() {
        let mot = "a".repeat(LIMITE_TEXTE * 2 + 7);
        let morceaux = decouper(&mot, LIMITE_TEXTE);
        println!(
            "mot de {} caractères -> {} morceaux de {:?}",
            mot.len(),
            morceaux.len(),
            morceaux
                .iter()
                .map(|m| longueur_utf16(m))
                .collect::<Vec<_>>()
        );
        assert_eq!(morceaux.len(), 3);
        assert_eq!(morceaux.iter().map(|m| m.len()).sum::<usize>(), mot.len());
    }

    #[test]
    fn les_erreurs_transitoires_se_distinguent_des_definitives() {
        let cas = [
            (429, Some(30), true, "débit dépassé"),
            (500, None, true, "panne côté Telegram"),
            (403, None, false, "bot bloqué par l'utilisateur"),
            (400, None, false, "message mal formé"),
            (401, None, false, "jeton révoqué"),
        ];
        for (code, retry_after, attendu, libelle) in cas {
            let erreur = ErreurEnvoi::Api {
                methode: "sendMessage",
                code,
                description: libelle.to_owned(),
                retry_after,
            };
            println!(
                "{code} {libelle:32} reprise={} attente={:?}",
                erreur.merite_une_reprise(),
                erreur.attendre()
            );
            assert_eq!(
                erreur.merite_une_reprise(),
                attendu,
                "mauvaise conduite sur {code}"
            );
        }
    }

    #[test]
    fn un_refus_de_telegram_se_deplie_en_erreur_parlante() {
        let brut = serde_json::json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 12",
            "parameters": { "retry_after": 12 }
        });
        let reponse: Reponse<MessageEnvoye> =
            serde_json::from_value(brut).expect("forme d'erreur de Telegram");
        let erreur = reponse
            .deplier("sendMessage")
            .expect_err("cette réponse est un refus");
        println!("erreur dépliée : {erreur}");
        println!("  reprise méritée : {}", erreur.merite_une_reprise());
        println!("  attente demandée : {:?} s", erreur.attendre());
        assert_eq!(erreur.attendre(), Some(12));
        assert!(erreur.merite_une_reprise());
    }
}
