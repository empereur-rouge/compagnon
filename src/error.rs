//! Erreurs exposées sur la surface HTTP, avec codes numériques stables.
//!
//! # Pourquoi des codes numériques
//!
//! Le webhook est appelé par Telegram, et demain par un tableau de bord d'exploitation. Un
//! appelant qui branche son comportement sur le **texte** d'un message casse dès qu'on
//! reformule une phrase. Le contrat est le code ; le message reste libre d'évoluer.
//!
//! Format transmis : `{"code": NNNN, "message": "..."}`.
//!
//! Le contrat vaut pour **toutes** les réponses d'erreur, y compris celles que produisent les
//! couches `tower` (délai dépassé, corps trop volumineux) et le routeur (route inconnue,
//! méthode non autorisée) — voir [`crate::http`], qui les enveloppe. Un contrat honoré
//! seulement par les gestionnaires ne serait pas un contrat.
//!
//! # Pourquoi des messages publics vagues
//!
//! La tranche `1xxx` renvoie un message volontairement imprécis. Le webhook est une adresse
//! publique : un appelant non authentifié ne doit pas pouvoir distinguer « secret absent » de
//! « secret erroné », sinon l'endpoint devient un oracle. Le détail réel part dans les
//! journaux via `tracing`, jamais sur le fil.
//!
//! La tranche `2xxx` fait exception : décrire un corps JSON illisible ne divulgue rien et fait
//! gagner du temps en intégration.
//!
//! # Numérotation partagée avec `agentbot`
//!
//! Les codes communs aux deux services portent délibérément la même valeur (`2004` = route
//! inconnue ici comme là-bas). Un exploitant qui tient les deux produits n'a qu'une grille à
//! connaître. Corollaire : **ne jamais réattribuer un code existant**. Un code retiré reste
//! retiré.

use std::borrow::Cow;
use std::error::Error as StdError;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::Level;

/// Codes d'erreur du service, regroupés par tranche fonctionnelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    // --- 1xxx : authentification / autorisation ---
    /// L'en-tête `X-Telegram-Bot-Api-Secret-Token` est absent, ou ne correspond pas.
    WebhookSecretInvalide,

    // --- 2xxx : validation de la requête ---
    /// Le corps n'est pas du JSON.
    PayloadIllisible,
    /// Le corps est du JSON, mais ne ressemble pas à une mise à jour Telegram.
    PayloadInattendu,
    /// Un paramètre de requête attendu manque.
    ParametreManquant,
    /// Aucune route ne correspond au chemin demandé.
    RouteInconnue,
    /// La route existe, la méthode HTTP non.
    MethodeNonAutorisee,
    /// Le corps dépasse [`crate::http::TAILLE_MAX_CORPS`].
    CorpsTropVolumineux,

    // --- 5xxx : ressources / quotas ---
    /// Telegram a refusé ou n'a pas répondu à un envoi lancé par le worker.
    ///
    /// Écrit dans `file_messages.erreur_derniere`, jamais rendu sur HTTP : c'est un code de
    /// tâche, pas de requête. Il vit dans la même grille pour qu'un exploitant n'ait qu'une
    /// table de codes à connaître.
    EnvoiImpossible,

    /// La charge utile d'une tâche n'a pas pu être relue.
    ///
    /// Signale une incohérence entre ce qui a été enfilé et ce que le worker sait lire —
    /// typiquement un déploiement à cheval sur deux versions du format.
    TacheIllisible,

    /// La file de traitement est pleine. **Ce n'est pas une défaillance** : c'est la
    /// contre-pression qui fonctionne, et le `503` demande à Telegram de repasser.
    FileSaturee,

    // --- 9xxx : interne ---
    /// Défaillance interne. Jamais de détail sur le fil.
    Interne,
    /// Le traitement a dépassé le délai imparti.
    DelaiDepasse,
}

impl ErrorCode {
    /// Tous les codes, pour les tests d'exhaustivité.
    ///
    /// Écrit à la main et non dérivé : c'est précisément ce qui fait échouer le test quand
    /// quelqu'un ajoute une variante sans lui donner de code.
    pub const TOUS: &'static [Self] = &[
        Self::WebhookSecretInvalide,
        Self::PayloadIllisible,
        Self::PayloadInattendu,
        Self::ParametreManquant,
        Self::RouteInconnue,
        Self::MethodeNonAutorisee,
        Self::CorpsTropVolumineux,
        Self::FileSaturee,
        Self::EnvoiImpossible,
        Self::TacheIllisible,
        Self::Interne,
        Self::DelaiDepasse,
    ];

    /// Valeur numérique transmise au client. Fait partie du contrat public.
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Self::WebhookSecretInvalide => 1001,
            Self::PayloadIllisible => 2001,
            Self::PayloadInattendu => 2002,
            Self::ParametreManquant => 2003,
            Self::RouteInconnue => 2004,
            Self::MethodeNonAutorisee => 2005,
            Self::CorpsTropVolumineux => 2006,
            Self::FileSaturee => 5001,
            Self::Interne => 9001,
            Self::EnvoiImpossible => 9003,
            Self::TacheIllisible => 9004,
            Self::DelaiDepasse => 9002,
        }
    }

    /// Message transmis au client. Volontairement vague sur les tranches sensibles.
    #[must_use]
    pub const fn message_public(self) -> &'static str {
        match self {
            Self::WebhookSecretInvalide => "requête non authentifiée",
            Self::PayloadIllisible => "corps de requête JSON invalide",
            Self::PayloadInattendu => "le corps ne correspond pas au format attendu",
            Self::ParametreManquant => "paramètre de requête manquant",
            Self::RouteInconnue => "route inconnue",
            Self::MethodeNonAutorisee => "méthode non autorisée pour cette route",
            Self::CorpsTropVolumineux => "corps de requête trop volumineux",
            // Vague à dessein : un appelant n'a pas à savoir si le service est saturé, ce qui
            // renseignerait qui cherche à le saturer.
            Self::FileSaturee => "requête refusée, réessayer plus tard",
            // Ces deux-là ne sortent jamais sur HTTP ; le message existe pour que la grille
            // reste complète et que `TOUS` puisse être éprouvé uniformément.
            Self::EnvoiImpossible | Self::TacheIllisible => "erreur interne",
            Self::Interne => "erreur interne",
            Self::DelaiDepasse => "délai de traitement dépassé",
        }
    }

    /// Statut HTTP associé.
    #[must_use]
    pub const fn statut(self) -> StatusCode {
        match self {
            Self::WebhookSecretInvalide => StatusCode::UNAUTHORIZED,
            Self::PayloadIllisible | Self::PayloadInattendu | Self::ParametreManquant => {
                StatusCode::BAD_REQUEST
            }
            Self::RouteInconnue => StatusCode::NOT_FOUND,
            Self::MethodeNonAutorisee => StatusCode::METHOD_NOT_ALLOWED,
            Self::CorpsTropVolumineux => StatusCode::PAYLOAD_TOO_LARGE,
            // 503 et non 429 : Telegram rejoue sur 5xx, ce qui est exactement le comportement
            // voulu. Un 429 le ferait abandonner selon sa propre politique.
            Self::FileSaturee => StatusCode::SERVICE_UNAVAILABLE,
            Self::EnvoiImpossible | Self::TacheIllisible => StatusCode::SERVICE_UNAVAILABLE,
            // 503 et non 500 : la défaillance est presque toujours transitoire. Telegram
            // rejoue la mise à jour sur 5xx, ce qui est exactement le comportement voulu —
            // le message de l'utilisateur ne doit pas disparaître parce que le disque était
            // saturé une seconde.
            Self::Interne | Self::DelaiDepasse => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Niveau de journalisation. Sépare ce qui mérite un regard de ce qui est du bruit.
    #[must_use]
    pub const fn niveau(self) -> Level {
        match self {
            Self::Interne | Self::DelaiDepasse => Level::ERROR,
            // Quelqu'un présente un mauvais secret : soit un déploiement mal configuré, soit
            // une sonde hostile. Les deux se regardent.
            // Une rafale de ces deux-là est le signal d'un déploiement désaccordé ou d'une
            // saturation durable : les deux se regardent, aucun n'est du bruit de fond.
            Self::WebhookSecretInvalide | Self::FileSaturee | Self::EnvoiImpossible => Level::WARN,
            Self::TacheIllisible => Level::ERROR,
            // Corps mal formés, routes inconnues : bruit de fond d'Internet.
            Self::PayloadIllisible
            | Self::PayloadInattendu
            | Self::ParametreManquant
            | Self::RouteInconnue
            | Self::MethodeNonAutorisee
            | Self::CorpsTropVolumineux => Level::DEBUG,
        }
    }
}

/// Une erreur en route vers le client, avec son détail interne attaché.
///
/// Le détail ne sort **jamais** sur le fil : il est journalisé au moment de la conversion en
/// réponse, et seul le [`ErrorCode::message_public`] part.
#[derive(Debug)]
pub struct ApiError {
    code: ErrorCode,
    detail: Cow<'static, str>,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl ApiError {
    /// Construit une erreur sans cause sous-jacente.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
            source: None,
        }
    }

    /// Construit une erreur en conservant la cause, pour la chaîne de diagnostic.
    #[must_use]
    pub fn avec_source(
        code: ErrorCode,
        detail: impl Into<Cow<'static, str>>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            detail: detail.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Le code porté par cette erreur.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Le détail interne et toute sa chaîne de causes, pour les journaux.
    #[must_use]
    pub fn diagnostic(&self) -> String {
        let mut texte = self.detail.to_string();
        let mut cause = self.source.as_ref().map(|s| s.as_ref() as &dyn StdError);
        while let Some(courante) = cause {
            texte.push_str(" ← ");
            texte.push_str(&courante.to_string());
            cause = courante.source();
        }
        texte
    }
}

/// Marqueur posé sur toute réponse produite par [`ApiError`].
///
/// # Pourquoi un marqueur plutôt qu'un reniflage de `content-type`
///
/// La couche d'enveloppe de [`crate::http`] doit distinguer une réponse d'erreur déjà conforme
/// au contrat d'une réponse nue produite par `tower` ou le routeur. Elle le faisait en
/// regardant si le `content-type` était `application/json` — une heuristique, pas une preuve :
/// un futur gestionnaire écrivant `(StatusCode::BAD_REQUEST, Json(...))` la traversait intact
/// et sortait du contrat sans qu'aucune ligne de code ne l'arrête.
///
/// Ce type est privé au crate et n'est inséré qu'ici. Sa présence ne peut donc venir que d'un
/// `ApiError`, qui ne sait produire qu'un corps conforme : la condition devient vraie par
/// construction au lieu d'être supposée.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DejaConforme;

/// Le corps JSON d'une réponse d'erreur.
#[derive(Debug, Serialize)]
pub(crate) struct CorpsErreur {
    pub(crate) code: u16,
    pub(crate) message: &'static str,
}

impl From<ErrorCode> for CorpsErreur {
    fn from(code: ErrorCode) -> Self {
        Self {
            code: code.code(),
            message: code.message_public(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Le diagnostic est journalisé ici et *uniquement* ici. Le niveau vient du registre ;
        // `tracing` exige un niveau constant à la macro, d'où l'aiguillage explicite.
        let code = self.code.code();
        let diagnostic = self.diagnostic();
        match self.code.niveau() {
            Level::ERROR => tracing::error!(code, detail = %diagnostic, "requête rejetée"),
            Level::WARN => tracing::warn!(code, detail = %diagnostic, "requête rejetée"),
            // Aucun code ne rend INFO aujourd'hui. Le bras reste parce que le `_` final
            // journaliserait un futur code INFO au niveau DEBUG, où personne ne le verrait.
            Level::INFO => tracing::info!(code, detail = %diagnostic, "requête rejetée"),
            _ => tracing::debug!(code, detail = %diagnostic, "requête rejetée"),
        }
        let mut reponse = (self.code.statut(), Json(CorpsErreur::from(self.code))).into_response();
        reponse.extensions_mut().insert(DejaConforme);
        reponse
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn les_codes_sont_uniques_et_dans_la_bonne_tranche() {
        let mut vus: Vec<u16> = Vec::new();
        for &c in ErrorCode::TOUS {
            let n = c.code();
            println!(
                "{:?} -> code={} statut={} niveau={} message={:?}",
                c,
                n,
                c.statut(),
                c.niveau(),
                c.message_public()
            );
            assert!(!vus.contains(&n), "code {n} attribué deux fois");
            vus.push(n);
            assert!(
                (1000..10_000).contains(&n),
                "code {n} hors des tranches 1xxx-9xxx"
            );
        }
        println!("\n{} codes, tous uniques et dans les tranches", vus.len());
    }

    #[test]
    fn aucun_detail_interne_ne_fuit_dans_le_message_public() {
        // Un détail interne réaliste : il contient le secret du webhook, ce qui serait
        // catastrophique s'il partait sur le fil.
        let secret = "secret-webhook-de-production-abc123";
        for &c in ErrorCode::TOUS {
            let erreur = ApiError::new(c, format!("secret attendu {secret}, reçu autre chose"));
            let public = c.message_public();
            println!(
                "{:?}\n    interne : {}\n    public  : {}",
                c,
                erreur.diagnostic(),
                public
            );
            assert!(
                !public.contains(secret),
                "{c:?} laisse fuir le secret dans son message public"
            );
            assert!(
                !public.contains("secret"),
                "{c:?} nomme le secret dans son message public"
            );
        }
        println!("\naucun des {} codes ne fuit", ErrorCode::TOUS.len());
    }

    #[test]
    fn la_chaine_de_diagnostic_remonte_les_causes() {
        #[derive(Debug, thiserror::Error)]
        #[error("le disque est plein")]
        struct Racine;

        let erreur = ApiError::avec_source(ErrorCode::Interne, "écriture impossible", Racine);
        let diagnostic = erreur.diagnostic();
        println!("diagnostic complet : {diagnostic}");
        assert_eq!(diagnostic, "écriture impossible ← le disque est plein");
    }
}
