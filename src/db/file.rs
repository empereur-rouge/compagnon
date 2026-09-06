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
//! Plusieurs workers consomment la même file. L'invariant à tenir est : **au plus une tâche en
//! vol par utilisateur**. Deux messages d'une même personne sont ainsi traités dans l'ordre,
//! pendant que les autres conversations avancent en parallèle.
//!
//! Cet invariant est tenu par un **index unique partiel**
//! (`idx_une_tache_en_vol_par_utilisateur`, migration `0002`), pas par la requête. C'est
//! délibéré, et c'est une correction : une première version l'exprimait par une composition de
//! `not exists` et de `pg_try_advisory_xact_lock` dans le `WHERE`, ce qui ne tenait pas —
//! PostgreSQL donne cette forme en contre-exemple, la correction dépendait du plan choisi
//! (mesuré : six workers servis avec un plan, un seul avec l'autre), et la course restait
//! ouverte en `read committed`. Voir l'en-tête de la migration.
//!
//! Deux mécanismes demeurent, chacun avec un rôle plus modeste et plus honnête :
//!
//! - **`for update skip locked`** : deux workers ne prennent jamais la même ligne, et aucun
//!   n'attend l'autre.
//! - **`not exists`** : filtre d'**efficacité**, qui évite la collision au lieu de la corriger.
//!   S'il laisse passer, l'index refuse, et le worker rejoue.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use super::ErreurBase;

/// Une tâche prise dans la file.
#[derive(Debug, sqlx::FromRow)]
pub struct Tache {
    /// Identifiant, à rendre à [`terminer`] ou [`echouer`].
    pub id: Uuid,
    /// À qui elle appartient. Sert aux journaux, et c'est la clé de sérialisation de la file.
    pub utilisateur_id: i64,
    /// Le contenu, tel qu'il a été enfilé.
    pub charge_utile: serde_json::Value,
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
/// La borne est **approximative sous concurrence**, et il faut le dire : deux insertions
/// simultanées prennent chacune leur instantané, comptent la même chose, et passent toutes
/// deux. Écrire le comptage et l'insertion dans une seule instruction ne sérialise rien — une
/// instruction n'est pas une section critique. Le dépassement est borné par le nombre
/// d'insertions vraiment concurrentes pour un même utilisateur, c'est-à-dire une poignée, ce
/// qui est sans conséquence pour une borne dont le rôle est d'empêcher une inondation. La
/// rendre exacte demanderait de sérialiser sur la ligne utilisateur, ce que le bénéfice ne
/// justifie pas ici.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue — notamment si l'utilisateur n'existe pas,
/// la clé étrangère étant ce qui garantit qu'aucune tâche n'est orpheline. Ou
/// [`ErreurBase::ChargeUtile`] si la valeur ne se convertit pas.
pub async fn enfiler(
    pool: &PgPool,
    utilisateur_id: i64,
    type_tache: &str,
    charge_utile: &impl serde::Serialize,
) -> Result<Option<Uuid>, ErreurBase> {
    // La conversion vit ici plutôt que chez l'appelant : c'est ce module qui possède le format
    // de la file, donc c'est à lui de répondre de ce qui y entre.
    let charge_utile = serde_json::to_value(charge_utile)?;
    Ok(sqlx::query_scalar(
        "insert into file_messages (utilisateur_id, type_tache, charge_utile)
         select $1, $2, $3
         where (select count(*) from file_messages
                where utilisateur_id = $1 and statut in ('en_attente', 'en_cours')) < $4
         returning id",
    )
    .bind(utilisateur_id)
    .bind(type_tache)
    .bind(&charge_utile)
    .bind(EN_FILE_MAX_PAR_UTILISATEUR)
    .fetch_optional(pool)
    .await?)
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
    let resultat = sqlx::query_as::<_, Tache>(
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
                       and a.bail_expire_le >= now())
             order by c.cree_le
             for update skip locked
             limit 1)
         returning f.id, f.utilisateur_id, f.charge_utile, f.tentatives",
    )
    .bind(secondes)
    .fetch_optional(pool)
    .await;

    match resultat {
        Ok(tache) => Ok(tache),
        // Un autre worker a pris une tâche de cet utilisateur entre notre filtre et notre
        // écriture. L'index a refusé : c'est exactement son rôle, et ce n'est pas une erreur —
        // c'est « rien à prendre pour l'instant ». L'appelant se repose et rejoue.
        Err(sqlx::Error::Database(erreur)) if erreur.is_unique_violation() => Ok(None),
        Err(erreur) => Err(ErreurBase::Requete(erreur)),
    }
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
    .await?;
    Ok(())
}

/// Nombre de prises au-delà duquel une tâche est abandonnée.
///
/// Vit ici et non chez le worker : c'est une politique de la file. Elle était portée par un
/// argument que chaque appelant choisissait, ce qui laissait un futur appelant écrire `7` et
/// changer la politique pour une classe de tâches sans que rien ne le signale.
pub const TENTATIVES_MAX: i16 = 3;

/// Rend une tâche après un échec : remise en attente, ou abandon si les reprises sont épuisées.
///
/// Le code d'erreur est un **entier stable**, pas un message. C'est délibéré et c'est la leçon
/// d'un incident de ce projet : écrire le `Display` d'une erreur dans une colonne, c'est y
/// écrire un jour un secret que l'erreur transportait à l'insu de tous.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn echouer(pool: &PgPool, id: Uuid, code: i32) -> Result<(), ErreurBase> {
    rendre(pool, id, code, TENTATIVES_MAX).await
}

/// Abandonne une tâche définitivement, sans lui laisser de reprise.
///
/// Pour les échecs dont on sait qu'ils se reproduiront à l'identique : une clé refusée, un
/// prompt qui n'a franchi aucun contrôle. Les rejouer épuise les tentatives en retardant le
/// moment où la personne apprend que ça ne marche pas.
///
/// Existe comme fonction plutôt que comme argument : la borne de reprise est une politique de
/// la file, pas un nombre que ses appelants choisissent. Passer `0` exprimait la bonne idée
/// dans le mauvais vocabulaire, et rien n'empêchait un futur appelant d'écrire `7`.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn abandonner(pool: &PgPool, id: Uuid, code: i32) -> Result<(), ErreurBase> {
    rendre(pool, id, code, 0).await
}

/// Le geste commun : rendre la tâche, en attente ou en échec selon la borne.
async fn rendre(
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
    .await?;
    Ok(())
}
