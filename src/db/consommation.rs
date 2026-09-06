//! La table `consommation` : une ligne par appel payant, et rien qui puisse la réécrire.
//!
//! # Ce qu'elle sert à répondre
//!
//! « Combien coûte réellement un utilisateur actif par mois ». C'est la question dont dépend
//! tout le prix de l'abonnement, elle se pose en phase 1.6, et elle ne peut pas être répondue
//! rétrospectivement : un coût qui n'a pas été inscrit au moment de l'appel est perdu. D'où une
//! table remplie dès la phase 1.3, alors qu'aucun quota n'existe encore.
//!
//! # Pourquoi des énumérations plutôt que des chaînes
//!
//! `type`, `origine` et `statut` sont des vocabulaires fermés, contraints par un `check` en
//! base. Passer des `&str` reporterait la faute de frappe à l'exécution, sur un chemin
//! d'écriture qui n'est jamais le chemin critique d'un test — c'est-à-dire là où elle serait
//! découverte le plus tard possible. Le vocabulaire est donc en Rust, et sa traduction SQL
//! tient en un endroit.
//!
//! # Ce que la table n'autorise pas
//!
//! Ni `update`, ni `delete` : la migration 0007 les refuse par trigger. La seule mutation
//! admise est l'anonymisation exigée par une purge RGPD, qui détache la ligne de son
//! utilisateur **sans en perdre le montant**. Ce module n'expose donc aucune écriture autre
//! qu'[`inscrire`], parce qu'il n'en existe aucune autre.

use std::time::Duration;

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use super::ErreurBase;

/// Ce qui a été produit, et donc facturé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeAppel {
    /// Une réponse textuelle.
    Message,
    /// Une image générée.
    Image,
    /// De la synthèse vocale.
    Audio,
    /// Une extraction de souvenirs, en tâche de fond.
    Extraction,
    /// Une compaction de l'historique, en tâche de fond.
    Compaction,
}

impl TypeAppel {
    /// La valeur acceptée par le `check` de la colonne `type`.
    const fn en_sql(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Extraction => "extraction",
            Self::Compaction => "compaction",
        }
    }
}

/// Ce qui a déclenché l'appel.
///
/// Les tâches de fond sont comptées comme le reste : elles ne consomment aucun quota
/// d'utilisateur mais coûtent, et une marge calculée sans elles est fausse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origine {
    /// L'utilisateur a écrit.
    Reponse,
    /// Le compagnon a pris l'initiative.
    Proactif,
    /// Aucun message n'attend le résultat.
    TacheFond,
}

impl Origine {
    /// La valeur acceptée par le `check` de la colonne `origine`.
    const fn en_sql(self) -> &'static str {
        match self {
            Self::Reponse => "reponse",
            Self::Proactif => "proactif",
            Self::TacheFond => "tache_fond",
        }
    }
}

/// Comment l'appel s'est terminé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Statut {
    /// Le fournisseur a rendu ce qui était demandé.
    Ok,
    /// L'appel a échoué. La ligne est inscrite quand même : un appel qui échoue après le début
    /// de la génération est souvent facturé, et un coût invisible ne se retrouve pas.
    Echec,
    /// La sortie a été produite puis écartée par la modération. Payée, donc comptée.
    RejeteModeration,
}

impl Statut {
    /// La valeur acceptée par le `check` de la colonne `statut`.
    const fn en_sql(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Echec => "echec",
            Self::RejeteModeration => "rejete_moderation",
        }
    }
}

/// Un appel payant, tel qu'il sera inscrit.
#[derive(Debug, Clone)]
pub struct Appel<'a> {
    /// À qui l'appel est imputé.
    pub utilisateur_id: i64,
    /// La conversation concernée, si l'appel en sert une.
    pub conversation_id: Option<Uuid>,
    /// Le message produit, `None` si l'appel a échoué avant qu'un message existe.
    pub message_id: Option<Uuid>,
    /// Ce qui a été produit.
    pub type_appel: TypeAppel,
    /// Ce qui a déclenché l'appel.
    pub origine: Origine,
    /// Le nom de l'hébergeur.
    pub fournisseur: &'a str,
    /// L'identifiant **exact** du modèle rendu par le fournisseur, pas celui demandé.
    pub modele: &'a str,
    /// Unités consommées en entrée, si le fournisseur les rend.
    pub unites_entree: Option<i32>,
    /// Unités produites, si le fournisseur les rend.
    pub unites_sortie: Option<i32>,
    /// Le coût, en euros, au tarif du moment.
    pub cout_eur: Decimal,
    /// La durée mesurée par l'appelant.
    pub duree: Option<Duration>,
    /// Comment l'appel s'est terminé.
    pub statut: Statut,
}

/// Inscrit un appel au registre, et rend l'identifiant de la ligne.
///
/// N'est **jamais** appelée dans la même transaction que l'appel au fournisseur : le pool est
/// dimensionné pour que personne ne tienne une connexion pendant une seconde de calcul GPU
/// (voir la constante `CONNEXIONS_MAX` de `db`).
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue — notamment si `utilisateur_id` ne désigne
/// personne, ce qui signalerait une imputation de coût à un inconnu.
pub async fn inscrire(pool: &PgPool, appel: &Appel<'_>) -> Result<Uuid, ErreurBase> {
    // Une durée dépassant 24,8 jours ne rentre pas dans un `integer`. Elle ne se produira pas,
    // mais la saturation vaut mieux qu'une troncature silencieuse qui inscrirait un chiffre
    // faux dans un registre dont toute la valeur est d'être exact.
    let duree_ms = appel.duree.map(|duree| {
        i32::try_from(duree.as_millis()).unwrap_or(i32::MAX)
    });

    Ok(sqlx::query_scalar(
        "insert into consommation
             (utilisateur_id, conversation_id, message_id, type, origine, fournisseur, modele,
              unites_entree, unites_sortie, cout_fournisseur_eur, duree_ms, statut)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         returning id",
    )
    .bind(appel.utilisateur_id)
    .bind(appel.conversation_id)
    .bind(appel.message_id)
    .bind(appel.type_appel.en_sql())
    .bind(appel.origine.en_sql())
    .bind(appel.fournisseur)
    .bind(appel.modele)
    .bind(appel.unites_entree)
    .bind(appel.unites_sortie)
    .bind(appel.cout_eur)
    .bind(duree_ms)
    .bind(appel.statut.en_sql())
    .fetch_one(pool)
    .await?)
}

/// Ce qu'un utilisateur a coûté depuis une date, en euros.
///
/// Existe dès maintenant parce que c'est **la** raison d'être de la table : sans lecture, on
/// accumule des lignes sans jamais répondre à la question qui a motivé leur écriture. Le
/// compteur par période de la phase 1.6 remplacera cette somme sur le chemin critique — pas
/// ici, où l'exactitude prime sur le coût de la requête.
///
/// Rend zéro si l'utilisateur n'a rien consommé, plutôt que `None` : « rien consommé » et
/// « a consommé zéro » sont la même réponse à cette question.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn cout_depuis(
    pool: &PgPool,
    utilisateur_id: i64,
    depuis: chrono::DateTime<chrono::Utc>,
) -> Result<Decimal, ErreurBase> {
    Ok(sqlx::query_scalar::<_, Option<Decimal>>(
        "select sum(cout_fournisseur_eur) from consommation
          where utilisateur_id = $1 and cree_le >= $2",
    )
    .bind(utilisateur_id)
    .bind(depuis)
    .fetch_one(pool)
    .await?
    .unwrap_or(Decimal::ZERO))
}
