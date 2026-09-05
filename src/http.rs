//! Routeur, état partagé, sonde de santé.
//!
//! # Le contrat d'erreur vaut pour tout, pas seulement pour les gestionnaires
//!
//! Une route inconnue, une méthode refusée, un corps trop volumineux, un délai dépassé : ces
//! réponses viennent du routeur et des couches `tower`, pas du code métier, et elles sortiraient
//! nues — un statut sans corps. Un appelant qui lit `{"code": ..., "message": ...}` partout
//! ailleurs tomberait alors sur une réponse vide, ce qui est précisément ce qu'un contrat est
//! censé empêcher. Une couche de ce module les rhabille avant qu'elles ne sortent.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::VERSION;
use crate::db::Base;
use crate::error::{ApiError, CorpsErreur, DejaConforme, ErrorCode};
use crate::horloge;
use crate::telegram::Canal;
use crate::webhook;

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
    /// La base : file de traitement, utilisateurs, et tout ce que la phase 1 y ajoutera.
    ///
    /// `PgPool` est interne­ment un `Arc` : cloner l'état à chaque requête reste trois
    /// incréments atomiques et zéro allocation, comme avec le canal mpsc qu'il remplace.
    pub base: Base,
    /// Compteur des consommateurs encore en vie, tenu par l'équipe.
    pub workers_vivants: Arc<AtomicUsize>,
    /// Quand le service a démarré, en secondes depuis l'époque Unix.
    pub demarre_le: i64,
}

/// Construit le routeur complet, couches comprises.
///
/// L'ordre des couches se lit de bas en haut à l'arrivée d'une requête : la trace est posée en
/// premier, l'enveloppe d'erreur ensuite, puis la limite de taille, puis le délai. La limite
/// vient avant le délai pour qu'un corps démesuré soit refusé sans consommer le délai imparti.
pub fn routeur(etat: EtatApp) -> Router {
    let webhook = Router::new()
        .route("/webhook", post(webhook::recevoir))
        // `route_layer` et non `layer` : la couche ne doit s'appliquer qu'à une route qui
        // existe. Posée en `layer`, un `POST /nimporte-quoi` exigerait un secret avant de
        // recevoir son 404, et l'endpoint deviendrait un moyen de distinguer les routes.
        .route_layer(axum::middleware::from_fn_with_state(
            etat.clone(),
            authentifier,
        ));

    Router::new()
        .route("/health", get(sante))
        .merge(webhook)
        .with_state(etat)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            DELAI_REQUETE,
        ))
        .layer(RequestBodyLimitLayer::new(TAILLE_MAX_CORPS))
        .layer(axum::middleware::from_fn(enveloppe_erreur))
        // Dernier rempart du contrat d'erreur. Sans lui, une panique dans un gestionnaire
        // coupe la connexion : Telegram voit une réinitialisation au lieu d'un 5xx conforme,
        // et rien n'est journalisé au format du contrat. Les lints `panic` et `unwrap_used`
        // sont à `warn` et non à `deny` — le cas est possible, pas hypothétique.
        .layer(CatchPanicLayer::custom(|_| {
            tracing::error!(
                code = ErrorCode::Interne.code(),
                "panique dans un gestionnaire, connexion préservée"
            );
            ApiError::new(ErrorCode::Interne, "panique dans un gestionnaire").into_response()
        }))
        .layer(TraceLayer::new_for_http().make_span_with(span_requete))
}

/// Vérifie que l'appel vient de Telegram, **avant** que le corps ne soit lu.
///
/// Posée en couche et non en première ligne du gestionnaire, pour une raison de fond : axum
/// exécute les extracteurs — dont [`axum::body::Bytes`], qui draine et collecte la requête —
/// puis seulement appelle le gestionnaire. Authentifier dans le corps laissait donc n'importe
/// qui imposer la lecture et l'allocation de [`TAILLE_MAX_CORPS`] sans présenter le moindre
/// secret, sur une adresse publique. Une couche ne voit que les en-têtes et court-circuite
/// avant — ce qui supprime au passage le clone intégral de la table d'en-têtes que
/// l'extracteur `HeaderMap` imposait à chaque requête.
///
/// # Errors
///
/// [`ErrorCode::WebhookSecretInvalide`] si le secret est absent, vide ou erroné — les trois
/// cas rendant le même code et le même message.
async fn authentifier(
    State(etat): State<EtatApp>,
    requete: Request,
    suite: Next,
) -> Result<Response, ApiError> {
    etat.canal.authentifier(requete.headers())?;
    Ok(suite.run(requete).await)
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

    // Le marqueur ne peut venir que d'un `ApiError`, qui ne sait produire qu'un corps
    // conforme. Renifler le `content-type` supposait qu'une réponse JSON est forcément
    // conforme — supposition qu'un futur gestionnaire renvoyant son propre `Json(...)`
    // aurait démentie en silence, sans qu'aucune ligne de code ne l'arrête.
    if reponse.extensions().get::<DejaConforme>().is_some() {
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
        // Seules les couches `tower` produisent un 503 nu ; la file saturée passe par un
        // `ApiError` qui porte déjà `FileSaturee` et n'atteint donc jamais cette table.
        StatusCode::SERVICE_UNAVAILABLE => ErrorCode::DelaiDepasse,
        _ => ErrorCode::Interne,
    }
}

/// Ce que `/health` renvoie.
///
/// Volontairement pauvre : cette adresse n'est pas authentifiée, elle sert au `HEALTHCHECK` du
/// conteneur. Rien de ce qu'elle expose n'apprend quoi que ce soit sur les utilisateurs.
/// Réversible — `Serialize` **et** `Deserialize` — plutôt que sérialisable seulement.
///
/// C'est ce qui permet à un test de lire des champs typés au lieu d'un blob indexé par chaîne.
/// La différence n'est pas cosmétique : `Value["file_libre"]` sur un champ supprimé rend `Null`
/// en silence, et c'est exactement ainsi que le test de cette sonde s'était mis à comparer
/// `Null` à `Null` — il passait quoi que le service renvoie. Un champ renommé est désormais une
/// erreur de compilation.
///
/// Le prix est deux `String` au lieu de deux `&'static str`, soit deux petites allocations par
/// appel de `/health` — une route qu'on interroge toutes les trente secondes, pas mille fois
/// par seconde.
#[derive(Debug, Serialize, Deserialize)]
pub struct Sante {
    /// `ok` tant que le service répond.
    pub statut: String,
    /// Version du binaire.
    pub version: String,
    /// Secondes écoulées depuis le démarrage.
    pub depuis: i64,
    /// Vrai si la base répond.
    ///
    /// La sonde reste à `200` même quand il vaut `false` : le service est joignable, et
    /// répondre `503` ferait retirer l'instance d'un équilibreur au moment précis où ses
    /// journaux sont la seule source de diagnostic. C'est le champ qu'un supervision doit
    /// lire, pas le code HTTP.
    pub base_repond: bool,
    /// Nombre de tâches encore à traiter, baux expirés compris.
    ///
    /// Une valeur qui croît sans redescendre est le signal que les consommateurs n'avancent
    /// plus — l'information la plus utile que cette sonde puisse porter. `null` si la base
    /// n'a pas répondu.
    pub taches_en_attente: Option<i64>,
    /// Nombre de consommateurs **encore en vie**, pour situer la valeur précédente.
    ///
    /// Ce qui tourne, et non ce qui a été lancé. La version précédente recopiait la constante
    /// — exactement la faute qu'un commentaire de ce fichier interdisait deux lignes plus haut
    /// avant d'être supprimé avec le code qu'il gardait. Un worker qui meurt doit se voir ici,
    /// sans quoi la sonde annonce quatre consommateurs devant une file qui n'avance plus.
    pub workers: usize,
}

/// Sonde de santé.
async fn sante(State(etat): State<EtatApp>) -> Json<Sante> {
    // Deux informations distinctes : la base répond-elle, et la file avance-t-elle. Une base
    // qui répond avec une file qui enfle est un cas bien plus fréquent qu'une base muette, et
    // les confondre en un seul booléen le rendrait indétectable.
    let taches_en_attente = etat.base.taches_en_attente().await.ok();
    let base_repond = taches_en_attente.is_some();
    Json(Sante {
        statut: "ok".to_owned(),
        version: VERSION.to_owned(),
        depuis: horloge::maintenant() - etat.demarre_le,
        base_repond,
        taches_en_attente,
        workers: etat.workers_vivants.load(Ordering::Relaxed),
    })
}
