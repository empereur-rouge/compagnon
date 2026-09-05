//! Les commandes d'exploitation qui touchent au compagnon.
//!
//! # Pourquoi des paires `clé=valeur`
//!
//! Créer un compagnon demande sept choix au minimum. En positionnel, une inversion entre deux
//! codes passerait inaperçue jusqu'à la lecture du prompt ; nommés, les arguments se relisent,
//! s'écrivent dans n'importe quel ordre, et un oubli se signale par son nom.
//!
//! Le dépôt n'a pas d'analyseur d'arguments en dépendance et n'en gagne pas un pour cela : ce
//! qui suit tient en vingt lignes.
//!
//! # Ce que ces commandes ne font pas
//!
//! Elles n'écrivent aucun prompt. La création pose des **choix**, puis appelle la validation —
//! qui compose, modère et inscrit. Il n'existe aucun chemin, ici ou ailleurs, par lequel un
//! texte saisi atteindrait le modèle.

use std::collections::HashMap;

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::db::{Base, ErreurBase, catalogues, utilisateurs};
use crate::personnage::{self, Cible, moderation};

/// Ce qui a empêché une commande d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurCompagnon {
    /// La base n'a pas répondu, ou a refusé.
    #[error("{0}")]
    Base(#[from] ErreurBase),

    /// Un argument manque, ou ne désigne rien.
    #[error("{0}")]
    Usage(String),
}

/// Découpe des arguments `clé=valeur` en table.
///
/// # Errors
///
/// [`ErreurCompagnon::Usage`] si un argument n'a pas la forme attendue.
fn en_champs(mots: &[&str]) -> Result<HashMap<String, String>, ErreurCompagnon> {
    mots.iter()
        .map(|mot| {
            mot.split_once('=')
                .map(|(cle, valeur)| (cle.to_owned(), valeur.to_owned()))
                .ok_or_else(|| {
                    ErreurCompagnon::Usage(format!("« {mot} » n'est pas de la forme clé=valeur"))
                })
        })
        .collect()
}

/// Un champ obligatoire.
fn exiger<'a>(champs: &'a HashMap<String, String>, cle: &str) -> Result<&'a str, ErreurCompagnon> {
    champs
        .get(cle)
        .map(String::as_str)
        .ok_or_else(|| ErreurCompagnon::Usage(format!("il manque {cle}=…")))
}

/// Affiche tout ce parmi quoi un compagnon peut être composé.
///
/// # Errors
///
/// [`ErreurCompagnon::Base`] si la base ne répond pas.
pub async fn montrer_catalogues(config: &Config) -> Result<(), ErreurCompagnon> {
    let base = Base::ouvrir(&config.url_base).await?;
    let pool = base.pool();

    for catalogue in catalogues::Catalogue::tous() {
        let options = catalogues::lister(pool, catalogue).await?;
        println!("\n{catalogue:?} ({} options)", options.len());
        for option in options {
            println!("  {:<24} {}", option.code, option.libelle);
        }
    }

    let tranches = catalogues::tranches_age(pool).await?;
    println!("\nTranchesAge ({} options)", tranches.len());
    for tranche in tranches {
        println!(
            "  {:<24} {} (à partir de {} ans)",
            tranche.code, tranche.libelle, tranche.age_min
        );
    }

    for (titre, traits) in [
        ("Archetypes", catalogues::archetypes(pool).await?),
        ("Tons", catalogues::tons(pool).await?),
    ] {
        println!("\n{titre} ({} options)", traits.len());
        for trait_ in traits {
            println!(
                "  {:<24} {} — {}",
                trait_.code, trait_.libelle, trait_.description
            );
        }
    }

    let curseurs = catalogues::parametres_gradues(pool).await?;
    println!(
        "\nCurseurs ({} options, valeurs de 0,00 à 1,00)",
        curseurs.len()
    );
    for curseur in curseurs {
        let plafonnable = if curseur.plafonnable_juridiction {
            " [plafonnable par pays]"
        } else {
            ""
        };
        println!(
            "  {:<24} {} — défaut {}{plafonnable}",
            curseur.code, curseur.libelle, curseur.valeur_defaut
        );
    }
    Ok(())
}

/// Crée un compagnon à partir de choix, puis le soumet à la modération.
///
/// # Errors
///
/// [`ErreurCompagnon`] si un champ manque, si un code ne désigne rien, ou si la base refuse.
pub async fn creer(config: &Config, mots: &[&str]) -> Result<(), ErreurCompagnon> {
    let champs = en_champs(mots)?;
    let base = Base::ouvrir(&config.url_base).await?;
    let pool = base.pool();

    let utilisateur: i64 = exiger(&champs, "utilisateur")?
        .parse()
        .map_err(|_| ErreurCompagnon::Usage("utilisateur= doit être un nombre".to_owned()))?;
    let nom = exiger(&champs, "nom")?;

    // L'utilisateur doit exister : sa clé étrangère le veut, et un compagnon sans propriétaire
    // n'aurait personne à qui parler.
    utilisateurs::assurer(pool, utilisateur, None).await?;

    let personnage_id: Uuid = sqlx::query_scalar(
        "insert into personnages (utilisateur_id, nom) values ($1, $2) returning id",
    )
    .bind(utilisateur)
    .bind(nom)
    .fetch_one(pool)
    .await
    .map_err(ErreurBase::Requete)?;

    poser_apparence(pool, personnage_id, &champs).await?;
    poser_traits(pool, personnage_id, &champs, Cible::Archetypes).await?;
    poser_traits(pool, personnage_id, &champs, Cible::Tons).await?;
    poser_curseurs(pool, personnage_id, &champs).await?;

    sqlx::query(
        "insert into personnage_parametres_interaction (personnage_id, longueur_reponse)
         values ($1, coalesce($2, 'moyenne'))",
    )
    .bind(personnage_id)
    .bind(champs.get("longueur").map(String::as_str))
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?;

    let pays: Option<String> =
        sqlx::query_scalar("select code_pays_declare from utilisateurs where id = $1")
            .bind(utilisateur)
            .fetch_one(pool)
            .await
            .map_err(ErreurBase::Requete)?;

    let modele = champs
        .get("modele")
        .map_or("a-definir-phase-1-3", String::as_str);
    let verdict = personnage::valider(pool, personnage_id, pays.as_deref(), modele).await?;

    match verdict {
        moderation::Verdict::Accepte => {
            println!("compagnon créé : {personnage_id}");
            println!("modération      : acceptée");
            println!("statut          : brouillon (activable)");
        }
        moderation::Verdict::Refuse(motif) => {
            println!("compagnon créé : {personnage_id}");
            println!("modération      : REFUSÉE — {}", motif.message_public());
            println!("statut          : rejeté, aucun prompt écrit");
        }
    }
    Ok(())
}

/// Affiche le prompt composé d'un compagnon.
///
/// # Errors
///
/// [`ErreurCompagnon`] si l'utilisateur n'a pas de compagnon, ou si la base refuse.
pub async fn montrer(config: &Config, utilisateur: &str) -> Result<(), ErreurCompagnon> {
    let utilisateur: i64 = utilisateur
        .parse()
        .map_err(|_| ErreurCompagnon::Usage("l'identifiant doit être un nombre".to_owned()))?;
    let base = Base::ouvrir(&config.url_base).await?;

    let ligne: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "select p.id, p.statut, u.code_pays_declare
           from personnages p join utilisateurs u on u.id = p.utilisateur_id
          where p.utilisateur_id = $1 and p.supprime_le is null",
    )
    .bind(utilisateur)
    .fetch_optional(base.pool())
    .await
    .map_err(ErreurBase::Requete)?;

    let Some((personnage_id, statut, pays)) = ligne else {
        return Err(ErreurCompagnon::Usage(format!(
            "l'utilisateur {utilisateur} n'a pas de compagnon"
        )));
    };

    let traits = personnage::charger(base.pool(), personnage_id, pays.as_deref()).await?;
    let prompt = personnage::composer(&traits);
    println!("compagnon {personnage_id} — statut {statut}");
    println!("empreinte {}", prompt.empreinte);
    println!("\n{}", prompt.texte);
    Ok(())
}

/// Enregistre une vérification d'âge.
///
/// Existe parce que la phase 1.1 l'a exigée sans donner aucun moyen de la poser : la seule
/// façon était une écriture SQL directe. Le parcours d'inscription la remplacera pour
/// l'utilisateur ; celle-ci reste pour le support.
///
/// # Errors
///
/// [`ErreurCompagnon`] si l'identifiant est illisible ou si la base refuse.
pub async fn verifier_age(config: &Config, utilisateur: &str) -> Result<(), ErreurCompagnon> {
    let utilisateur: i64 = utilisateur
        .parse()
        .map_err(|_| ErreurCompagnon::Usage("l'identifiant doit être un nombre".to_owned()))?;
    let base = Base::ouvrir(&config.url_base).await?;
    utilisateurs::verifier_age(base.pool(), utilisateur, "declaration").await?;
    println!("utilisateur {utilisateur} : âge vérifié (méthode « declaration »)");
    eprintln!(
        "note : la déclaration simple ne suffit pas dans les juridictions qui exigent une \
         vérification robuste — voir DECISIONS-MODELES et la revue légale par pays."
    );
    Ok(())
}

async fn poser_apparence(
    pool: &PgPool,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
) -> Result<(), ErreurCompagnon> {
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
    .bind(exiger(champs, "genre")?)
    .bind(exiger(champs, "age")?)
    .bind(exiger(champs, "morphologie")?)
    .bind(champs.get("cheveux").map(String::as_str))
    .bind(champs.get("longueur_cheveux").map(String::as_str))
    .bind(champs.get("yeux").map(String::as_str))
    .bind(champs.get("style").map(String::as_str))
    .execute(pool)
    .await
    .map_err(|erreur| {
        // Un code inconnu rend `null` dans un `select`, donc échoue sur la contrainte
        // `not null` — le message brut de PostgreSQL ne dirait pas lequel.
        ErreurCompagnon::Usage(format!(
            "un code d'apparence ne désigne rien au catalogue ({erreur}). \
             Voir « compagnon catalogues »."
        ))
    })?;
    Ok(())
}

async fn poser_traits(
    pool: &PgPool,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
    cible: Cible,
) -> Result<(), ErreurCompagnon> {
    let prefixe = cible.prefixe();
    let principal = exiger(champs, prefixe)?;
    poser_un_trait(pool, personnage_id, cible, principal, "principal", None).await?;

    for rang in 1_i16..=2 {
        if let Some(code) = champs.get(&format!("{prefixe}{}", rang + 1)) {
            poser_un_trait(pool, personnage_id, cible, code, "secondaire", Some(rang)).await?;
        }
    }
    Ok(())
}

/// Pose un trait, en refusant un code qui ne désigne rien au catalogue.
///
/// La `Cible` porte les trois noms de table, qui voyagent toujours ensemble : passés
/// séparément, ils faisaient huit arguments et rien n'empêchait de mélanger la table de liaison
/// des archétypes avec la référence des tons.
async fn poser_un_trait(
    pool: &PgPool,
    personnage_id: Uuid,
    cible: Cible,
    code: &str,
    role: &str,
    rang: Option<i16>,
) -> Result<(), ErreurCompagnon> {
    let (liaison, reference, colonne) = cible.tables();
    let touchees = sqlx::query(&format!(
        "insert into {liaison} (personnage_id, {colonne}, role, rang)
         select $1, id, $3, $4 from {reference} where code = $2 and actif"
    ))
    .bind(personnage_id)
    .bind(code)
    .bind(role)
    .bind(rang)
    .execute(pool)
    .await
    .map_err(ErreurBase::Requete)?
    .rows_affected();

    if touchees == 0 {
        return Err(ErreurCompagnon::Usage(format!(
            "« {code} » ne désigne rien dans {reference}. Voir « compagnon catalogues »."
        )));
    }
    Ok(())
}

async fn poser_curseurs(
    pool: &PgPool,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
) -> Result<(), ErreurCompagnon> {
    for curseur in catalogues::parametres_gradues(pool).await? {
        let valeur = match champs.get(&curseur.code) {
            Some(brut) => brut.parse::<Decimal>().map_err(|_| {
                ErreurCompagnon::Usage(format!(
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
        .execute(pool)
        .await
        .map_err(|_| {
            ErreurCompagnon::Usage(format!(
                "{} = {valeur} sort des bornes acceptées (0,00 à 1,00)",
                curseur.code
            ))
        })?;
    }
    Ok(())
}
