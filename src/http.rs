//! Routeur, état partagé, sonde de santé.
//!
//! # Le contrat d'erreur vaut pour tout, pas seulement pour les gestionnaires
//!
//! Une route inconnue, une méthode refusée, un corps trop volumineux, un délai dépassé : ces
//! réponses viennent du routeur et des couches `tower`, pas du code métier, et elles sortiraient
//! nues — un statut sans corps. Un appelant qui lit `{"code": ..., "message": ...}` partout
//! ailleurs tomberait alors sur une réponse vide, ce qui est précisément ce qu'un contrat est
//! censé empêcher. Une couche de ce module les rhabille avant qu'elles ne sortent.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::mpsc;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::VERSION;
use crate::error::{CorpsErreur, ErrorCode};
use crate::horloge;
use crate::telegram::Canal;
use crate::telegram::types::Recu;
use crate::webhook;
use crate::worker::CAPACITE_FILE;

/// Taille maximale d'un corps de requête.
///
/// Une mise à jour Telegram tient très largement dedans : le plus gros message texte fait
/// 4096 caractères, et les médias arrivent par référence, jamais en ligne. La limite existe
/// pour qu'un corps de dix mégaoctets soit refusé par `tower` avant d'atteindre `serde`.
pub const TAILLE_MAX_CORPS: usize = 256 * 1024;

/// Délai au-delà duquel une requête entrante est abandonnée.
///
/// Le gestionnaire de webhook ne fait qu'authentifier, lire et enfiler : s'il dépasse cinq
/// secondes, quelque chose est bloqué et laisser la requête pendre n'y changera rien.
const DELAI_REQUETE: Duration = Duration::from_secs(5);

/// L'état que chaque requête reçoit.
///
/// Ne dérive pas `Debug` : il porte un [`Canal`], qui porte le jeton du bot.
#[derive(Clone)]
pub struct EtatApp {
    /// Le canal Telegram, partagé.
    pub canal: Arc<Canal>,
    /// L'entrée de la file de traitement.
    pub expediteur: mpsc::Sender<Recu>,
    /// Quand le service a démarré, en secondes depuis l'époque Unix.
    pub demarre_le: i64,
}

impl EtatApp {
    /// Assemble l'état.
    #[must_use]
    pub const fn new(canal: Arc<Canal>, expediteur: mpsc::Sender<Recu>, demarre_le: i64) -> Self {
        Self {
            canal,
            expediteur,
            demarre_le,
        }
    }
}

/// Construit le routeur complet, couches comprises.
///
/// L'ordre des couches se lit de bas en haut à l'arrivée d'une requête : la trace est posée en
/// premier, l'enveloppe d'erreur ensuite, puis la limite de taille, puis le délai. La limite
/// vient avant le délai pour qu'un corps démesuré soit refusé sans consommer le délai imparti.
pub fn routeur(etat: EtatApp) -> Router {
    Router::new()
        .route("/health", get(sante))
        .route("/webhook", post(webhook::recevoir))
        .with_state(etat)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            DELAI_REQUETE,
        ))
        .layer(RequestBodyLimitLayer::new(TAILLE_MAX_CORPS))
        .layer(axum::middleware::from_fn(enveloppe_erreur))
        .layer(TraceLayer::new_for_http().make_span_with(span_requete))
}

/// Le span posé sur chaque requête.
///
/// Ni la chaîne de requête ni les en-têtes n'y figurent : le secret du webhook voyage dans un
/// en-tête, et une trace qui le recopierait le publierait dans les journaux.
fn span_requete(requete: &Request) -> tracing::Span {
    tracing::info_span!(
        "http",
        methode = %requete.method(),
        chemin = %requete.uri().path(),
    )
}

/// Rhabille les réponses d'erreur produites hors des gestionnaires.
async fn enveloppe_erreur(requete: Request, suite: Next) -> Response {
    let reponse = suite.run(requete).await;
    let statut = reponse.status();
    if !statut.is_client_error() && !statut.is_server_error() {
        return reponse;
    }

    // Une réponse déjà JSON vient d'un `ApiError` : elle porte le bon code, ne pas la refaire.
    let deja_json = reponse
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|valeur| valeur.to_str().ok())
        .is_some_and(|valeur| valeur.starts_with("application/json"));
    if deja_json {
        return reponse;
    }

    let code = code_pour_statut(statut);
    tracing::debug!(
        statut = statut.as_u16(),
        code = code.code(),
        "réponse d'erreur sans corps enveloppée"
    );
    (statut, Json(CorpsErreur::from(code))).into_response()
}

/// Associe un code à un statut produit hors des gestionnaires.
const fn code_pour_statut(statut: StatusCode) -> ErrorCode {
    match statut {
        StatusCode::BAD_REQUEST => ErrorCode::ParametreManquant,
        StatusCode::UNAUTHORIZED => ErrorCode::WebhookSecretInvalide,
        StatusCode::NOT_FOUND => ErrorCode::RouteInconnue,
        StatusCode::METHOD_NOT_ALLOWED => ErrorCode::MethodeNonAutorisee,
        StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::CorpsTropVolumineux,
        StatusCode::SERVICE_UNAVAILABLE => ErrorCode::DelaiDepasse,
        _ => ErrorCode::Interne,
    }
}

/// Ce que `/health` renvoie.
///
/// Volontairement pauvre : cette adresse n'est pas authentifiée, elle sert au `HEALTHCHECK` du
/// conteneur. Rien de ce qu'elle expose n'apprend quoi que ce soit sur les utilisateurs.
#[derive(Debug, Serialize)]
pub struct Sante {
    /// `ok` tant que le service répond.
    pub statut: &'static str,
    /// Version du binaire.
    pub version: &'static str,
    /// Secondes écoulées depuis le démarrage.
    pub depuis: i64,
    /// Places encore libres dans la file de traitement.
    ///
    /// Un zéro durable est le signal que le worker n'avance plus — l'information la plus utile
    /// que cette sonde puisse porter.
    pub file_libre: usize,
    /// Capacité totale de la file, pour situer `file_libre`.
    pub file_capacite: usize,
}

/// Sonde de santé.
async fn sante(State(etat): State<EtatApp>) -> Json<Sante> {
    Json(Sante {
        statut: "ok",
        version: VERSION,
        depuis: horloge::maintenant() - etat.demarre_le,
        file_libre: etat.expediteur.capacity(),
        file_capacite: CAPACITE_FILE,
    })
}

/// L'adresse effective sur laquelle un écouteur est lié.
///
/// Passe par une fonction plutôt que par un champ recopié : lier sur le port `0` donne un port
/// éphémère que seul le système connaît, et c'est celui-là que les tests doivent joindre.
///
/// # Errors
///
/// Renvoie l'erreur d'entrée-sortie si l'écouteur ne sait pas dire son adresse.
pub fn adresse_liee(ecoute: &tokio::net::TcpListener) -> std::io::Result<SocketAddr> {
    ecoute.local_addr()
}
