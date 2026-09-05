//! La table `utilisateurs` : celui qui écrit, et ce à quoi il a droit.
//!
//! L'identifiant est celui de Telegram, jamais généré ici. Il est stable, unique, et connu dès
//! le premier message : en fabriquer un second créerait deux identités pour une personne, et
//! toute la mémoire de la phase 2 pendrait de la mauvaise.

use sqlx::PgPool;

use super::ErreurBase;

/// Crée l'utilisateur s'il est inconnu, met son prénom à jour **s'il a changé**.
///
/// Appelé sur le chemin d'entrée de chaque message : c'est ce qui garantit que la clé
/// étrangère de `file_messages` est satisfiable au moment d'enfiler.
///
/// La clause `where` du `do update` n'est pas une optimisation : sans elle, chaque message
/// déclenchait une écriture de ligne et le trigger d'horodatage, faisant dire à
/// `mis_a_jour_le` « dernier message reçu » au lieu de « dernière modification ». La migration
/// justifie ce trigger par « une colonne d'audit à laquelle on ne peut pas se fier ne vaut
/// rien » — c'est exactement ce qu'elle serait devenue.
///
/// Le prénom n'est écrasé que s'il est fourni : Telegram peut l'omettre, et remplacer une
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
            set prenom_affiche = excluded.prenom_affiche
          where excluded.prenom_affiche is not null
            and utilisateurs.prenom_affiche is distinct from excluded.prenom_affiche",
    )
    .bind(id)
    .bind(prenom)
    .execute(pool)
    .await?;
    Ok(())
}

/// Vrai si l'utilisateur a passé la vérification d'âge.
///
/// Appelée par le worker, qui est celui qui parle à l'utilisateur : un refus doit produire un
/// message, et un silence serait indiscernable d'une panne. Le prix est un aller-retour de
/// plus par tâche, et il est assumé pour cette phase — la porte d'admission (`crate::admission`)
/// est l'endroit où ce contrôle descendra quand elle portera aussi le bannissement et le quota.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue. Un utilisateur inconnu rend `false` — c'est
/// le défaut sûr : on n'accorde pas un accès à quelqu'un dont on ne sait rien.
pub async fn age_verifie(pool: &PgPool, id: i64) -> Result<bool, ErreurBase> {
    Ok(sqlx::query_scalar::<_, bool>(
        "select age_verifie_le is not null
         from utilisateurs where id = $1 and supprime_le is null",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}

/// Enregistre une vérification d'âge.
///
/// La méthode est contrainte en base (`declaration`, `prestataire_tiers`, `document`) et la
/// date ne peut pas exister sans elle : une vérification sans méthode serait inauditable, ce
/// qui est le seul usage réel de cette colonne.
///
/// Inscrit l'utilisateur s'il est inconnu, plutôt que de ne toucher aucune ligne en silence.
/// C'est la forme qu'appelle l'inscription de la phase 1.2 — et celle dont le harnais de test
/// avait besoin, ce qui l'avait poussé à réécrire ce SQL à la main. Une seule définition du
/// geste, employée des deux côtés.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue, notamment si la méthode n'est pas reconnue.
pub async fn verifier_age(pool: &PgPool, id: i64, methode: &str) -> Result<(), ErreurBase> {
    sqlx::query(
        "insert into utilisateurs (id, age_verifie_le, methode_verification_age)
         values ($1, now(), $2)
         on conflict (id) do update
            set age_verifie_le = now(), methode_verification_age = excluded.methode_verification_age",
    )
    .bind(id)
    .bind(methode)
    .execute(pool)
    .await?;
    Ok(())
}

/// Le pays déclaré par l'utilisateur, s'il en a déclaré un.
///
/// Déclaratif et non déduit d'une géolocalisation : cela évite de construire un système de
/// profilage de localisation, et c'est cohérent avec la vérification d'âge. C'est lui qui
/// détermine les plafonds de juridiction.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn pays_declare(pool: &PgPool, id: i64) -> Result<Option<String>, ErreurBase> {
    Ok(
        sqlx::query_scalar("select code_pays_declare from utilisateurs where id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}
