//! Les écritures qui composent un compagnon.
//!
//! # Pourquoi ce module existe
//!
//! Ces fonctions vivaient dans le module de ligne de commande, en `fn` privées. Deux
//! conséquences, dont la seconde s'était déjà produite :
//!
//! - le parcours d'inscription depuis Telegram (phase 1.3) n'aurait pu réutiliser aucune de ces
//!   écritures, et les aurait recopiées ;
//! - **les tests les avaient déjà recopiées**, en omettant le `and actif` que la production
//!   applique. Ils construisaient donc des compagnons sur des lignes de catalogue désactivées,
//!   que la production refuse — et aucun test ne pouvait attraper une régression sur ce filtre,
//!   qui est pourtant le mécanisme de retrait dont la migration 0003 fait un argument de sûreté.
//!
//! C'est le même défaut que le harnais avait corrigé en phase 1.1 avec `verifier_age`, refait
//! un cran plus loin. Une seule définition de chaque écriture, appelée par la ligne de commande
//! comme par les tests.
//!
//! # Ce que ces fonctions ne font pas
//!
//! Elles posent des **choix**. Aucune n'écrit de prompt, aucune ne décide d'une validation :
//! c'est [`crate::personnage::valider`] qui compose, modère et inscrit — et il n'existe aucun
//! chemin par lequel un texte saisi atteindrait le modèle.
//!
//! Toutes prennent une transaction : les sept écritures d'une création sont un tout. Sans cela,
//! un échec au milieu laissait la ligne `personnages` committée, et l'index unique interdisait
//! toute nouvelle tentative pour cet utilisateur.

use std::collections::HashMap;

use rust_decimal::Decimal;
use uuid::Uuid;

use super::{ErreurBase, catalogues};
use crate::personnage::Cible;

/// Ce qui a empêché une écriture d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurEcriture {
    /// La base a refusé, ou n'a pas répondu.
    #[error("{0}")]
    Base(#[from] ErreurBase),

    /// Un code ne désigne rien au catalogue, ou une valeur sort de ses bornes.
    ///
    /// Distinguée d'une panne : sans cette séparation, une connexion perdue était annoncée à
    /// l'exploitant comme sa faute de frappe.
    #[error("{0}")]
    Saisie(String),
}

/// Vrai si cette erreur vient de ce que l'utilisateur a tapé, et non d'une panne.
///
/// Les trois codes retenus sont ceux qu'une saisie fautive produit : violation de `not null`
/// (un code qui ne désigne rien rend `null`), de clé étrangère, et de contrainte `check`.
fn faute_de_saisie(erreur: &sqlx::Error) -> bool {
    erreur
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| matches!(code.as_ref(), "23502" | "23503" | "23514"))
}

/// Crée la ligne du compagnon et rend son identifiant.
///
/// # Errors
///
/// [`ErreurEcriture::Base`] si l'écriture échoue — notamment si l'utilisateur a déjà un
/// compagnon, l'index unique l'interdisant.
pub async fn creer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    utilisateur: Uuid,
    nom: &str,
) -> Result<Uuid, ErreurEcriture> {
    Ok(sqlx::query_scalar(
        "insert into personnages (utilisateur_id, nom) values ($1, $2) returning id",
    )
    .bind(utilisateur)
    .bind(nom)
    .fetch_one(&mut **tx)
    .await
    .map_err(ErreurBase::Requete)?)
}

/// Pose l'apparence : genre, tranche d'âge et morphologie sont obligatoires, le reste non.
///
/// Chaque code est résolu **et** filtré sur `actif` dans la même instruction : un aller-retour
/// au lieu d'une lecture puis d'une écriture, et une option retirée du catalogue est refusée
/// sans qu'aucune vérification préalable n'ait à y penser.
///
/// # Errors
///
/// [`ErreurEcriture::Saisie`] si un code ne désigne rien d'actif, [`ErreurEcriture::Base`] pour
/// toute autre défaillance.
pub async fn poser_apparence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
) -> Result<(), ErreurEcriture> {
    sqlx::query(
        "insert into personnage_apparence
            (personnage_id, genre_id, tranche_age_id, morphologie_id, couleur_cheveux_id,
             longueur_cheveux, couleur_yeux_id, style_vestimentaire_id)
         select $1,
                (select id from ref_genres where code = $2 and actif),
                (select id from ref_tranches_age_apparent where code = $3 and actif),
                (select id from ref_morphologies where code = $4 and actif),
                (select id from ref_couleurs_cheveux where code = $5 and actif),
                $6,
                (select id from ref_couleurs_yeux where code = $7 and actif),
                (select id from ref_styles_vestimentaires where code = $8 and actif)",
    )
    .bind(personnage_id)
    .bind(exige(champs, "genre")?)
    .bind(exige(champs, "age")?)
    .bind(exige(champs, "morphologie")?)
    .bind(champs.get("cheveux").map(String::as_str))
    .bind(champs.get("longueur_cheveux").map(String::as_str))
    .bind(champs.get("yeux").map(String::as_str))
    .bind(champs.get("style").map(String::as_str))
    .execute(&mut **tx)
    .await
    .map_err(|erreur| {
        // Un code inconnu rend `null` dans un `select`, donc échoue sur la contrainte
        // `not null`. On distingue ce cas d'une vraie panne plutôt que de tout annoncer comme
        // une faute de frappe — et on n'interpole SURTOUT pas le `Display` de l'erreur : la
        // migration 0001 dit pourquoi, et ce chemin a déjà été emprunté une fois dans ce projet.
        if faute_de_saisie(&erreur) {
            ErreurEcriture::Saisie(
                "un code d'apparence ne désigne rien au catalogue. \
                 Voir « compagnon catalogues »."
                    .to_owned(),
            )
        } else {
            ErreurEcriture::Base(ErreurBase::Requete(erreur))
        }
    })?;
    Ok(())
}

/// Pose un trait principal et jusqu'à deux secondaires, pour l'une des deux compositions.
///
/// Les clés lues sont `archetype`, `archetype2`, `archetype3` — ou `ton`, `ton2`, `ton3` selon
/// la [`Cible`].
///
/// # Errors
///
/// [`ErreurEcriture::Saisie`] si le principal manque ou si un code ne désigne rien d'actif.
pub async fn poser_traits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
    cible: Cible,
) -> Result<(), ErreurEcriture> {
    let prefixe = cible.prefixe();
    let principal = champs
        .get(prefixe)
        .map(String::as_str)
        .ok_or_else(|| ErreurEcriture::Saisie(format!("il manque {prefixe}=…")))?;
    poser_un_trait(tx, personnage_id, cible, principal, "principal", None).await?;

    for rang in 1_i16..=2 {
        if let Some(code) = champs.get(&format!("{prefixe}{}", rang + 1)) {
            poser_un_trait(tx, personnage_id, cible, code, "secondaire", Some(rang)).await?;
        }
    }
    Ok(())
}

/// Pose un trait, en refusant un code qui ne désigne rien au catalogue.
///
/// La `Cible` porte les trois noms de table, qui voyagent toujours ensemble : passés
/// séparément, ils faisaient huit arguments et rien n'empêchait de mélanger la table de liaison
/// des archétypes avec la référence des tons.
/// Pose un trait, en refusant un code qui ne désigne rien d'actif au catalogue.
///
/// La `Cible` porte les trois noms de table, qui voyagent toujours ensemble : passés
/// séparément ils faisaient huit arguments, et rien n'empêchait de mélanger la table de liaison
/// des archétypes avec la référence des tons.
///
/// # Errors
///
/// [`ErreurEcriture::Saisie`] si le code ne désigne rien d'actif.
pub async fn poser_un_trait(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    cible: Cible,
    code: &str,
    role: &str,
    rang: Option<i16>,
) -> Result<(), ErreurEcriture> {
    let (liaison, reference, colonne) = cible.tables();
    let touchees = sqlx::query(&format!(
        "insert into {liaison} (personnage_id, {colonne}, role, rang)
         select $1, id, $3, $4 from {reference} where code = $2 and actif"
    ))
    .bind(personnage_id)
    .bind(code)
    .bind(role)
    .bind(rang)
    .execute(&mut **tx)
    .await
    .map_err(ErreurBase::Requete)?
    .rows_affected();

    if touchees == 0 {
        return Err(ErreurEcriture::Saisie(format!(
            "« {code} » ne désigne rien dans {reference}. Voir « compagnon catalogues »."
        )));
    }
    Ok(())
}

/// Pose tous les curseurs portés par le compagnon, en retombant sur les défauts du catalogue.
///
/// Ceux portés par l'**utilisateur** — `intensite_suggestive` — sont ignorés : la base les
/// refuse, et deux sources de vérité pour le seul paramètre à conséquence légale seraient pire
/// qu'un oubli.
///
/// # Errors
///
/// [`ErreurEcriture::Saisie`] si une valeur n'est pas un nombre, ou sort de `[0, 1]`.
pub async fn poser_curseurs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
) -> Result<(), ErreurEcriture> {
    for curseur in catalogues::parametres_gradues_dans(tx).await? {
        // Les curseurs portés par l'utilisateur ne se posent pas sur un compagnon : la base le
        // refuse désormais, et l'ignorer ici évite d'aller se le faire dire.
        if curseur.porte_par != "compagnon" {
            continue;
        }
        let valeur = match champs.get(&curseur.code) {
            Some(brut) => brut.parse::<Decimal>().map_err(|_| {
                ErreurEcriture::Saisie(format!(
                    "{}= doit être un nombre entre 0 et 1",
                    curseur.code
                ))
            })?,
            None => curseur.valeur_defaut,
        };
        sqlx::query(
            "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
             values ($1, $2, $3)",
        )
        .bind(personnage_id)
        .bind(&curseur.code)
        .bind(valeur)
        .execute(&mut **tx)
        .await
        .map_err(|erreur| {
            if faute_de_saisie(&erreur) {
                ErreurEcriture::Saisie(format!(
                    "{} = {valeur} sort des bornes acceptées (0,00 à 1,00)",
                    curseur.code
                ))
            } else {
                ErreurEcriture::Base(ErreurBase::Requete(erreur))
            }
        })?;
    }
    Ok(())
}

/// Un champ obligatoire de la table de choix.
fn exige<'a>(champs: &'a HashMap<String, String>, cle: &str) -> Result<&'a str, ErreurEcriture> {
    champs
        .get(cle)
        .map(String::as_str)
        .ok_or_else(|| ErreurEcriture::Saisie(format!("il manque {cle}=…")))
}
