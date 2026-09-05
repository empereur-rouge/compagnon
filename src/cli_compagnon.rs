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

/// Vrai si cette erreur vient de ce que l'utilisateur a tapé, et non d'une panne.
///
/// Sans cette distinction, une connexion perdue ou un délai de pool était annoncé à
/// l'exploitant comme sa faute de frappe — c'est la discrimination *state-divergence vs
/// transient* du projet, prise à l'envers.
///
/// Les trois codes retenus sont ceux qu'une saisie fautive produit : violation de `not null`
/// (un code qui ne désigne rien rend `null`), de clé étrangère, et de contrainte `check`.
fn faute_de_saisie(erreur: &sqlx::Error) -> bool {
    erreur
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| matches!(code.as_ref(), "23502" | "23503" | "23514"))
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

    // Les sept écritures qui suivent sont UNE transaction, et ce n'est pas du zèle : sans elle,
    // un échec au milieu — un code d'archétype mal tapé — laissait la ligne `personnages`
    // committée. L'index unique interdisait alors toute nouvelle tentative pour cet utilisateur,
    // qui se retrouvait avec un compagnon vide et aucun moyen d'en créer un autre.
    let mut tx = pool.begin().await.map_err(ErreurBase::Requete)?;

    let personnage_id: Uuid = sqlx::query_scalar(
        "insert into personnages (utilisateur_id, nom) values ($1, $2) returning id",
    )
    .bind(utilisateur)
    .bind(nom)
    .fetch_one(&mut *tx)
    .await
    .map_err(ErreurBase::Requete)?;

    poser_apparence(&mut tx, personnage_id, &champs).await?;
    poser_traits(&mut tx, personnage_id, &champs, Cible::Archetypes).await?;
    poser_traits(&mut tx, personnage_id, &champs, Cible::Tons).await?;
    poser_curseurs(&mut tx, personnage_id, &champs).await?;

    sqlx::query("insert into personnage_parametres_interaction (personnage_id) values ($1)")
        .bind(personnage_id)
        .execute(&mut *tx)
        .await
        .map_err(ErreurBase::Requete)?;
    if let Some(longueur) = champs.get("longueur") {
        // Écrit seulement s'il est fourni : la colonne porte déjà son défaut, et le recopier
        // ici en aurait fait une seconde définition ignorable.
        sqlx::query(
            "update personnage_parametres_interaction set longueur_reponse = $2
              where personnage_id = $1",
        )
        .bind(personnage_id)
        .bind(longueur)
        .execute(&mut *tx)
        .await
        .map_err(ErreurBase::Requete)?;
    }

    personnage::inscrire_version(&mut tx, personnage_id, "creation").await?;
    tx.commit().await.map_err(ErreurBase::Requete)?;

    let pays = utilisateurs::pays_declare(pool, utilisateur).await?;

    let modele = champs
        .get("modele")
        .map_or("a-definir-phase-1-3", String::as_str);
    let verdict = personnage::valider(pool, personnage_id, pays.as_deref(), modele).await?;

    match verdict {
        moderation::Verdict::Accepte => {
            println!("compagnon créé : {personnage_id}");
            println!("modération      : acceptée");
            println!("statut          : brouillon");
            println!("pour l'activer  : compagnon compagnon activer {utilisateur}");
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
    let (base, personnage_id, pays) = compagnon_de(config, utilisateur).await?;
    let statut: String = sqlx::query_scalar("select statut from personnages where id = $1")
        .bind(personnage_id)
        .fetch_one(base.pool())
        .await
        .map_err(ErreurBase::Requete)?;

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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .execute(&mut **tx)
    .await
    .map_err(|erreur| {
        // Un code inconnu rend `null` dans un `select`, donc échoue sur la contrainte
        // `not null`. On distingue ce cas d'une vraie panne plutôt que de tout annoncer comme
        // une faute de frappe — et on n'interpole SURTOUT pas le `Display` de l'erreur : la
        // migration 0001 dit pourquoi, et ce chemin a déjà été emprunté une fois dans ce projet.
        if faute_de_saisie(&erreur) {
            ErreurCompagnon::Usage(
                "un code d'apparence ne désigne rien au catalogue. \
                 Voir « compagnon catalogues »."
                    .to_owned(),
            )
        } else {
            ErreurCompagnon::Base(ErreurBase::Requete(erreur))
        }
    })?;
    Ok(())
}

async fn poser_traits(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
    cible: Cible,
) -> Result<(), ErreurCompagnon> {
    let prefixe = cible.prefixe();
    let principal = exiger(champs, prefixe)?;
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
async fn poser_un_trait(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .execute(&mut **tx)
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    champs: &HashMap<String, String>,
) -> Result<(), ErreurCompagnon> {
    for curseur in catalogues::parametres_gradues_dans(tx).await? {
        // Les curseurs portés par l'utilisateur ne se posent pas sur un compagnon : la base le
        // refuse désormais, et l'ignorer ici évite d'aller se le faire dire.
        if curseur.porte_par != "compagnon" {
            continue;
        }
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
        .execute(&mut **tx)
        .await
        .map_err(|erreur| {
            if faute_de_saisie(&erreur) {
                ErreurCompagnon::Usage(format!(
                    "{} = {valeur} sort des bornes acceptées (0,00 à 1,00)",
                    curseur.code
                ))
            } else {
                ErreurCompagnon::Base(ErreurBase::Requete(erreur))
            }
        })?;
    }
    Ok(())
}

/// Active le compagnon d'un utilisateur, si la modération l'a validé.
///
/// # Errors
///
/// [`ErreurCompagnon`] si l'utilisateur n'a pas de compagnon, ou si le prompt n'est pas validé —
/// le déclencheur de la base refuse alors, et c'est lui qui a le dernier mot.
pub async fn activer(config: &Config, utilisateur: &str) -> Result<(), ErreurCompagnon> {
    let (base, personnage_id, _) = compagnon_de(config, utilisateur).await?;
    personnage::activer(base.pool(), personnage_id).await?;

    let statut: String = sqlx::query_scalar("select statut from personnages where id = $1")
        .bind(personnage_id)
        .fetch_one(base.pool())
        .await
        .map_err(ErreurBase::Requete)?;
    println!("compagnon {personnage_id} — statut {statut}");
    Ok(())
}

/// Vérifie que le prompt validé dit encore ce que les traits composent.
///
/// # Errors
///
/// [`ErreurCompagnon`] si l'utilisateur n'a pas de compagnon, ou si la base refuse.
pub async fn verifier(config: &Config, utilisateur: &str) -> Result<(), ErreurCompagnon> {
    let (base, personnage_id, pays) = compagnon_de(config, utilisateur).await?;
    let etat = personnage::verifier_integrite(base.pool(), personnage_id, pays.as_deref()).await?;
    println!("compagnon {personnage_id} : {etat:?}");
    match etat {
        personnage::Integrite::Intacte => println!("  le prompt validé décrit bien ce compagnon"),
        personnage::Integrite::TexteAltere => {
            println!("  le texte stocké ne correspond plus à son empreinte : ligne altérée");
        }
        personnage::Integrite::DeriveDepuisValidation => {
            println!("  les traits ou le catalogue ont changé depuis la validation ;");
            println!("  revalider avant d'activer.");
        }
        personnage::Integrite::PasDePromptValide => println!("  rien à vérifier"),
    }
    Ok(())
}

/// Le compagnon d'un utilisateur, avec sa base et le pays déclaré.
///
/// Trois commandes posaient la même question de trois façons ; celle-ci la pose une fois.
async fn compagnon_de(
    config: &Config,
    utilisateur: &str,
) -> Result<(Base, Uuid, Option<String>), ErreurCompagnon> {
    let utilisateur: i64 = utilisateur
        .parse()
        .map_err(|_| ErreurCompagnon::Usage("l'identifiant doit être un nombre".to_owned()))?;
    let base = Base::ouvrir(&config.url_base).await?;

    let ligne: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "select p.id, u.code_pays_declare
           from personnages p join utilisateurs u on u.id = p.utilisateur_id
          where p.utilisateur_id = $1 and p.supprime_le is null",
    )
    .bind(utilisateur)
    .fetch_optional(base.pool())
    .await
    .map_err(ErreurBase::Requete)?;

    let Some((personnage_id, pays)) = ligne else {
        return Err(ErreurCompagnon::Usage(format!(
            "l'utilisateur {utilisateur} n'a pas de compagnon"
        )));
    };
    Ok((base, personnage_id, pays))
}
