//! La table `utilisateurs` : celui qui écrit, et ce à quoi il a droit.
//!
//! # L'identité n'est plus celle de Telegram
//!
//! Elle l'a été, et l'argument tenait : l'identifiant Telegram est stable, unique, connu dès le
//! premier message, et n'invente pas une seconde identité. Ce qu'il ne permet pas, c'est qu'une
//! même personne existe sur deux canaux — et le jour où sortir de Telegram devient une décision
//! plutôt qu'une option, l'identifiant du canal cesse d'être une identité : il redevient ce
//! qu'il est, une adresse.
//!
//! `utilisateurs.id` est donc un UUID interne, et `identifiants_externes` fait le pont. La
//! résolution `(canal, identifiant externe) → utilisateur` est le **premier** traitement de
//! toute requête entrante, quel que soit le canal ; au-delà, plus rien ne connaît Telegram,
//! hormis `messages.identifiant_telegram` et l'adresse de réponse portée par la charge utile.
//!
//! La bascule a été faite en phase 1.3, alors que Telegram était encore l'unique canal. C'était
//! la dernière fenêtre où elle ne coûtait presque rien : la phase 1.5 ajoute l'onboarding et la
//! 1.6 les abonnements, c'est-à-dire deux familles de tables de plus indexées par utilisateur.

use sqlx::PgPool;
use uuid::Uuid;

use super::ErreurBase;

/// Le canal Telegram, tel qu'il est inscrit dans `identifiants_externes`.
///
/// Constante plutôt que littéral : c'est la valeur d'un vocabulaire contraint en base, et le
/// jour où un second canal arrive, la liste des canaux se lit d'un seul endroit.
pub const CANAL_TELEGRAM: &str = "telegram";

/// Résout `(canal, identifiant externe)` vers un utilisateur, en le créant s'il est inconnu.
///
/// Le premier traitement de toute requête entrante. Rend l'UUID interne, seule chose que le
/// reste du service manipule.
///
/// # Pourquoi une seule requête, et pourquoi ce `on conflict`
///
/// Deux personnes peuvent écrire pour la première fois en même temps, et deux workers peuvent
/// traiter la même personne au redémarrage. Une lecture suivie d'une écriture ouvrirait une
/// course que l'index unique `(canal, identifiant_externe)` transformerait en erreur, sur le
/// chemin d'entrée d'un message — donc devant quelqu'un. Le `do update` sans effet est ce qui
/// permet un `returning` sur le chemin de conflit, et rend l'appel idempotent.
///
/// Le prénom n'est écrasé que s'il est fourni et qu'il a changé : Telegram peut l'omettre,
/// remplacer une valeur connue par du vide serait une régression silencieuse, et écrire à
/// chaque message ferait dire à `mis_a_jour_le` « dernier message reçu » au lieu de « dernière
/// modification ». La migration 0001 justifie ce trigger par « une colonne d'audit à laquelle
/// on ne peut pas se fier ne vaut rien » — c'est exactement ce qu'elle serait devenue.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si une écriture échoue.
pub async fn resoudre(
    pool: &PgPool,
    canal: &str,
    identifiant_externe: &str,
    prenom: Option<&str>,
) -> Result<Uuid, ErreurBase> {
    let mut tx = pool.begin().await?;

    let connu: Option<Uuid> = sqlx::query_scalar(
        "select utilisateur_id from identifiants_externes
          where canal = $1 and identifiant_externe = $2",
    )
    .bind(canal)
    .bind(identifiant_externe)
    .fetch_optional(&mut *tx)
    .await?;

    let id = match connu {
        Some(id) => {
            sqlx::query(
                "update utilisateurs set prenom_affiche = $2
                  where id = $1 and $2 is not null and prenom_affiche is distinct from $2",
            )
            .bind(id)
            .bind(prenom)
            .execute(&mut *tx)
            .await?;
            id
        }
        None => {
            let id: Uuid =
                sqlx::query_scalar("insert into utilisateurs (prenom_affiche) values ($1) returning id")
                    .bind(prenom)
                    .fetch_one(&mut *tx)
                    .await?;
            // `do update` sans effet plutôt que `do nothing` : sur conflit — deux premières
            // requêtes simultanées — il faut récupérer l'identité déjà posée, pas rendre zéro
            // ligne. L'utilisateur créé juste au-dessus devient alors orphelin, sans compagnon
            // ni conversation ; la transaction est annulée dans ce cas.
            let retenu: Uuid = sqlx::query_scalar(
                "insert into identifiants_externes (utilisateur_id, canal, identifiant_externe)
                 values ($1, $2, $3)
                 on conflict (canal, identifiant_externe)
                 do update set canal = excluded.canal
                 returning utilisateur_id",
            )
            .bind(id)
            .bind(canal)
            .bind(identifiant_externe)
            .fetch_one(&mut *tx)
            .await?;

            if retenu != id {
                // Quelqu'un d'autre a gagné la course : sa ligne fait foi, et l'utilisateur
                // qu'on venait de créer n'a jamais servi à rien.
                tx.rollback().await?;
                return Ok(retenu);
            }
            id
        }
    };

    tx.commit().await?;
    Ok(id)
}

/// Résout un identifiant Telegram, en créant l'utilisateur s'il est inconnu.
///
/// Enveloppe de [`resoudre`] pour le seul canal existant aujourd'hui. Elle porte la conversion
/// en texte, qui n'a aucune raison d'être refaite par chaque appelant : `identifiant_externe`
/// est du texte parce que le prochain canal n'aura aucune raison d'en fournir un numérique.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si une écriture échoue.
pub async fn resoudre_telegram(
    pool: &PgPool,
    identifiant: i64,
    prenom: Option<&str>,
) -> Result<Uuid, ErreurBase> {
    resoudre(pool, CANAL_TELEGRAM, &identifiant.to_string(), prenom).await
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
pub async fn age_verifie(pool: &PgPool, id: Uuid) -> Result<bool, ErreurBase> {
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
pub async fn verifier_age(pool: &PgPool, id: Uuid, methode: &str) -> Result<(), ErreurBase> {
    sqlx::query(
        "update utilisateurs
            set age_verifie_le = now(), methode_verification_age = $2
          where id = $1",
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
pub async fn pays_declare(pool: &PgPool, id: Uuid) -> Result<Option<String>, ErreurBase> {
    Ok(
        sqlx::query_scalar("select code_pays_declare from utilisateurs where id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}
