//! La file de traitement, en base et à bail.
//!
//! # Ce que le bail règle
//!
//! Un simple état « en cours » ne survit pas à la mort du worker qui l'a posé : la tâche reste
//! marquée prise par personne, et rien ne la reprend jamais. Le bail est une **échéance** —
//! passée celle-ci, la tâche redevient prenable. Aucun nettoyage périodique n'est nécessaire :
//! la requête de prise inclut les baux expirés dans ses candidats.
//!
//! # Ordre et concurrence
//!
//! Plusieurs workers consomment la même file. Deux règles se combinent :
//!
//! - **`for update skip locked`** : deux workers ne prennent jamais la même ligne, et aucun
//!   n'attend l'autre.
//! - **sérialisation par utilisateur** : la requête écarte tout utilisateur ayant déjà une
//!   tâche en vol. Deux messages d'une même personne sont donc traités dans l'ordre, pendant
//!   que les autres conversations avancent en parallèle.
//!
//! La seconde ne suffirait pas seule : deux workers pourraient constater simultanément
//! qu'un utilisateur est libre. Le verrou consultatif `pg_try_advisory_xact_lock` ferme cette
//! course — il ne vit que le temps de la transaction de prise, pas celui du traitement, ce qui
//! évite de tenir une connexion ouverte pendant un appel de modèle.

use std::time::Duration;

use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use super::ErreurBase;

/// Une tâche prise dans la file.
#[derive(Debug)]
pub struct Tache {
    /// Identifiant, à rendre à [`terminer`] ou [`echouer`].
    pub id: Uuid,
    /// À qui elle appartient — c'est aussi la clé de sérialisation.
    pub utilisateur_id: i64,
    /// Le contenu, tel qu'il a été enfilé.
    pub charge_utile: serde_json::Value,
    /// Ce qu'il faut en faire.
    pub type_tache: String,
    /// Nombre de prises, celle-ci comprise. Sert à borner les reprises.
    pub tentatives: i16,
}

/// Nombre de tâches non traitées qu'un même utilisateur peut avoir en file.
///
/// La file en mémoire de la phase 0 était bornée par construction ; une table ne l'est pas, et
/// une file qui grossit sans limite transforme un afflux en disque plein.
///
/// La borne est **par utilisateur** et non globale, parce qu'une borne globale se retourne
/// contre les mauvaises personnes : un seul émetteur en rafale la remplirait et ferait refuser
/// tous les autres. Trente-deux est très au-delà de ce qu'une conversation réelle produit —
/// personne n'a trente-deux messages sans réponse — et bien en deçà de ce qui coûte.
pub const EN_FILE_MAX_PAR_UTILISATEUR: i64 = 32;

/// Ajoute une tâche à la file, sauf si l'utilisateur a déjà atteint sa borne.
///
/// Rend `None` quand la borne est atteinte. Ce n'est pas une erreur : c'est de la
/// contre-pression, et l'appelant doit la traduire par un refus que Telegram rejouera.
///
/// L'insertion et le comptage sont **une seule instruction** : faites en deux, deux messages
/// simultanés du même utilisateur pourraient tous deux constater qu'il reste de la place.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue — notamment si l'utilisateur n'existe pas,
/// la clé étrangère étant ce qui garantit qu'aucune tâche n'est orpheline.
pub async fn enfiler(
    pool: &PgPool,
    utilisateur_id: i64,
    type_tache: &str,
    charge_utile: &serde_json::Value,
) -> Result<Option<Uuid>, ErreurBase> {
    let ligne = sqlx::query(
        "insert into file_messages (utilisateur_id, type_tache, charge_utile)
         select $1, $2, $3
         where (select count(*) from file_messages
                where utilisateur_id = $1 and statut in ('en_attente', 'en_cours')) < $4
         returning id",
    )
    .bind(utilisateur_id)
    .bind(type_tache)
    .bind(charge_utile)
    .bind(EN_FILE_MAX_PAR_UTILISATEUR)
    .fetch_optional(pool)
    .await
    .map_err(ErreurBase::Requete)?;

    ligne
        .map(|l| l.try_get("id").map_err(ErreurBase::Requete))
        .transpose()
}

/// Prend une tâche, ou rend `None` s'il n'y a rien de prenable.
///
/// `None` ne signifie pas « la file est vide » : il peut rester des tâches dont l'utilisateur
/// est déjà servi ailleurs. C'est le comportement voulu — l'appelant réessaie.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la base refuse la requête.
pub async fn prendre(pool: &PgPool, bail: Duration) -> Result<Option<Tache>, ErreurBase> {
    let secondes = i64::try_from(bail.as_secs()).unwrap_or(i64::MAX);
    let ligne = sqlx::query(
        "update file_messages f
         set statut = 'en_cours',
             bail_expire_le = now() + make_interval(secs => $1::bigint),
             tentatives = tentatives + 1
         where f.id = (
             select c.id from file_messages c
             where (c.statut = 'en_attente'
                    or (c.statut = 'en_cours' and c.bail_expire_le < now()))
               and not exists (
                     select 1 from file_messages a
                     where a.utilisateur_id = c.utilisateur_id
                       and a.statut = 'en_cours'
                       and a.bail_expire_le >= now()
                       and a.id <> c.id)
               and pg_try_advisory_xact_lock(c.utilisateur_id)
             order by c.cree_le
             for update skip locked
             limit 1)
         returning f.id, f.utilisateur_id, f.charge_utile, f.type_tache, f.tentatives",
    )
    .bind(secondes)
    .fetch_optional(pool)
    .await
    .map_err(ErreurBase::Requete)?;

    ligne
        .map(|l| {
            Ok(Tache {
                id: l.try_get("id").map_err(ErreurBase::Requete)?,
                utilisateur_id: l.try_get("utilisateur_id").map_err(ErreurBase::Requete)?,
                charge_utile: l.try_get("charge_utile").map_err(ErreurBase::Requete)?,
                type_tache: l.try_get("type_tache").map_err(ErreurBase::Requete)?,
                tentatives: l.try_get("tentatives").map_err(ErreurBase::Requete)?,
            })
        })
        .transpose()
}

/// Marque une tâche comme traitée.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn terminer(pool: &PgPool, id: Uuid) -> Result<(), ErreurBase> {
    sqlx::query(
        "update file_messages set statut = 'traite', traite_le = now(), bail_expire_le = null
         where id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?;
    Ok(())
}

/// Rend une tâche après un échec : remise en attente, ou abandon si les reprises sont épuisées.
///
/// Le code d'erreur est un **entier stable**, pas un message. C'est délibéré et c'est la leçon
/// d'un incident de ce projet : écrire le `Display` d'une erreur dans une colonne, c'est y
/// écrire un jour un secret que l'erreur transportait à l'insu de tous.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn echouer(
    pool: &PgPool,
    id: Uuid,
    code: i32,
    tentatives_max: i16,
) -> Result<(), ErreurBase> {
    sqlx::query(
        "update file_messages
         set statut = case when tentatives >= $3 then 'echec' else 'en_attente' end,
             erreur_derniere = $2,
             bail_expire_le = null,
             traite_le = case when tentatives >= $3 then now() else null end
         where id = $1",
    )
    .bind(id)
    .bind(code)
    .bind(tentatives_max)
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?;
    Ok(())
}
