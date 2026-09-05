//! La table `utilisateurs` : celui qui écrit, et ce à quoi il a droit.
//!
//! L'identifiant est celui de Telegram, jamais généré ici. Il est stable, unique, et connu dès
//! le premier message : en fabriquer un second créerait deux identités pour une personne, et
//! toute la mémoire de la phase 2 pendrait de la mauvaise.

use sqlx::{PgPool, Row as _};

use super::ErreurBase;

/// Crée l'utilisateur s'il est inconnu, met son prénom à jour s'il a changé.
///
/// Appelé sur le chemin d'entrée de chaque message : c'est ce qui garantit que la clé
/// étrangère de `file_messages` est satisfiable au moment d'enfiler.
///
/// Le prénom n'est mis à jour que s'il est fourni — Telegram peut l'omettre, et écraser une
/// valeur connue par du vide serait une régression silencieuse de la donnée.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn assurer(pool: &PgPool, id: i64, prenom: Option<&str>) -> Result<(), ErreurBase> {
    sqlx::query(
        "insert into utilisateurs (id, prenom_affiche)
         values ($1, $2)
         on conflict (id) do update
            set prenom_affiche = coalesce(excluded.prenom_affiche, utilisateurs.prenom_affiche)",
    )
    .bind(id)
    .bind(prenom)
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?;
    Ok(())
}

/// Vrai si l'utilisateur a passé la vérification d'âge.
///
/// # Pourquoi cette question est posée ici et pas dans le worker
///
/// Elle est posée **avant d'enfiler**. Un message qui ne pourra jamais produire de réponse n'a
/// rien à faire dans la file : l'y mettre ferait payer sa place et son traitement, et ferait
/// attendre derrière lui les messages de gens qui, eux, ont le droit d'être servis.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue. Un utilisateur inconnu rend `false` — c'est
/// le défaut sûr : on n'accorde pas un accès à quelqu'un dont on ne sait rien.
pub async fn age_verifie(pool: &PgPool, id: i64) -> Result<bool, ErreurBase> {
    let ligne = sqlx::query(
        "select age_verifie_le is not null as verifie
         from utilisateurs where id = $1 and supprime_le is null",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(ErreurBase::Requete)?;

    match ligne {
        Some(l) => l.try_get("verifie").map_err(ErreurBase::Requete),
        None => Ok(false),
    }
}

/// Enregistre une vérification d'âge.
///
/// La méthode est contrainte en base (`declaration`, `prestataire_tiers`, `document`) et la
/// date ne peut pas exister sans elle : une vérification sans méthode serait inauditable, ce
/// qui est le seul usage réel de cette colonne.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue, notamment si la méthode n'est pas reconnue.
pub async fn verifier_age(pool: &PgPool, id: i64, methode: &str) -> Result<(), ErreurBase> {
    sqlx::query(
        "update utilisateurs
         set age_verifie_le = now(), methode_verification_age = $2
         where id = $1 and supprime_le is null",
    )
    .bind(id)
    .bind(methode)
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?;
    Ok(())
}
