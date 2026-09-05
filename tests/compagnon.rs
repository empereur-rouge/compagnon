//! Ce que la base refuse de faire d'un compagnon.
//!
//! # Pourquoi ces tests portent sur des refus
//!
//! Les tables `personnage_*` ne contiennent presque pas de logique : ce qu'elles apportent, ce
//! sont des états qu'elles rendent **impossibles**. Un test qui vérifie qu'une insertion
//! correcte réussit ne dit rien ; ce qui protège, c'est de constater qu'une insertion fautive
//! échoue — quel que soit le chemin d'écriture, y compris ceux qu'aucun code Rust n'emprunte.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::db::Base;
use harnais::base::BaseDeTest;
use sqlx::PgPool;
use uuid::Uuid;

const COMPAGNON_A: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const COMPAGNON_B: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

/// Deux utilisateurs, chacun avec son compagnon en brouillon.
async fn deux_compagnons() -> (BaseDeTest, Base) {
    let jetable = BaseDeTest::creer().await;
    let base = Base::ouvrir(&jetable.url).await.expect("base migrée");
    sqlx::query("insert into utilisateurs (id) values (1), (2)")
        .execute(base.pool())
        .await
        .expect("utilisateurs");
    sqlx::query(
        "insert into personnages (id, utilisateur_id, nom) values ($1, 1, 'Léa'), ($2, 2, 'Nour')",
    )
    .bind(COMPAGNON_A)
    .bind(COMPAGNON_B)
    .execute(base.pool())
    .await
    .expect("compagnons");
    (jetable, base)
}

/// Pose un archétype sur un compagnon, en désignant le code du catalogue.
async fn poser_archetype(
    pool: &PgPool,
    compagnon: Uuid,
    code: &str,
    role: &str,
    rang: Option<i16>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into personnage_archetypes (personnage_id, archetype_id, role, rang)
         select $1, id, $3, $4 from ref_archetypes where code = $2",
    )
    .bind(compagnon)
    .bind(code)
    .bind(role)
    .bind(rang)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn un_compagnon_ne_peut_pas_s_activer_sans_prompt_valide() {
    let (jetable, base) = deux_compagnons().await;

    // C'est la dernière garantie avant qu'un compagnon ne se mette à parler : elle porte tout
    // ce que la modération aura décidé. La spécification la disait « vérifiable par une requête
    // d'audit » — vérifiable n'est pas tenu, et ce test éprouve qu'elle est désormais tenue.

    let sans_prompt = sqlx::query("update personnages set statut = 'actif' where id = $1")
        .bind(COMPAGNON_A)
        .execute(base.pool())
        .await;
    println!(
        "activation sans prompt du tout   -> {}",
        if sans_prompt.is_err() {
            "REFUSÉE"
        } else {
            "acceptée"
        }
    );
    assert!(
        sans_prompt.is_err(),
        "un compagnon sans prompt s'est activé"
    );

    sqlx::query(
        "insert into personnage_parametres_modele
            (personnage_id, prompt_systeme_genere, prompt_systeme_hash, modele_cible)
         values ($1, 'un prompt quelconque', 'empreinte', 'modele-x')",
    )
    .bind(COMPAGNON_A)
    .execute(base.pool())
    .await
    .expect("prompt posé");

    let non_valide = sqlx::query("update personnages set statut = 'actif' where id = $1")
        .bind(COMPAGNON_A)
        .execute(base.pool())
        .await;
    println!(
        "activation, prompt non validé    -> {}",
        if non_valide.is_err() {
            "REFUSÉE"
        } else {
            "acceptée"
        }
    );
    assert!(
        non_valide.is_err(),
        "un prompt écrit mais non modéré a suffi à activer"
    );

    sqlx::query(
        "update personnage_parametres_modele set valide_le = now() where personnage_id = $1",
    )
    .bind(COMPAGNON_A)
    .execute(base.pool())
    .await
    .expect("validation");

    sqlx::query("update personnages set statut = 'actif' where id = $1")
        .bind(COMPAGNON_A)
        .execute(base.pool())
        .await
        .expect("l'activation doit passer une fois la modération faite");
    let statut: String = sqlx::query_scalar("select statut from personnages where id = $1")
        .bind(COMPAGNON_A)
        .fetch_one(base.pool())
        .await
        .expect("lecture");
    println!("activation après modération      -> {statut}");
    assert_eq!(statut, "actif");

    jetable.detruire().await;
}

#[tokio::test]
async fn la_personnalite_reste_dans_ses_bornes() {
    let (jetable, base) = deux_compagnons().await;

    // Un principal obligatoire, jusqu'à deux secondaires : les bornes du document, tenues à
    // l'écriture. La résolution du prompt lit cette forme et n'a pas de branche pour une autre.
    poser_archetype(base.pool(), COMPAGNON_A, "timide", "principal", None)
        .await
        .expect("le premier principal passe");
    let second = poser_archetype(base.pool(), COMPAGNON_A, "calme", "principal", None).await;
    println!(
        "second archétype principal   -> {}",
        if second.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(
        second.is_err(),
        "deux principaux : la résolution ne saurait lequel prendre"
    );

    poser_archetype(base.pool(), COMPAGNON_A, "dominant", "secondaire", Some(1))
        .await
        .expect("premier secondaire");
    poser_archetype(base.pool(), COMPAGNON_A, "joueur", "secondaire", Some(2))
        .await
        .expect("second secondaire");
    let troisieme = poser_archetype(base.pool(), COMPAGNON_A, "loyal", "secondaire", Some(3)).await;
    println!(
        "troisième secondaire         -> {}",
        if troisieme.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(troisieme.is_err());

    // Et les deux incohérences de forme, que la contrainte croisée attrape.
    let principal_range =
        poser_archetype(base.pool(), COMPAGNON_B, "calme", "principal", Some(1)).await;
    println!(
        "principal avec un rang       -> {}",
        if principal_range.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(principal_range.is_err());

    let secondaire_sans_rang =
        poser_archetype(base.pool(), COMPAGNON_B, "loyal", "secondaire", None).await;
    println!(
        "secondaire sans rang         -> {}",
        if secondaire_sans_rang.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(secondaire_sans_rang.is_err());

    jetable.detruire().await;
}

#[tokio::test]
async fn une_conversation_ne_peut_pas_pointer_le_compagnon_d_un_autre() {
    let (jetable, base) = deux_compagnons().await;

    // Trois index uniques garantissaient trois bornes indépendantes, et aucun ne garantissait
    // que la conversation d'un utilisateur pointe SON compagnon : les deux clés étrangères de
    // `conversations` étaient disjointes. Sur un produit intime, c'est le chemin par lequel la
    // mémoire de quelqu'un atterrirait chez un autre.
    let croisee =
        sqlx::query("insert into conversations (utilisateur_id, personnage_id) values (1, $1)")
            .bind(COMPAGNON_B)
            .execute(base.pool())
            .await;
    println!(
        "conversation de 1 vers le compagnon de 2 -> {}",
        if croisee.is_err() {
            "REFUSÉE"
        } else {
            "acceptée"
        }
    );
    assert!(
        croisee.is_err(),
        "le triangle est ouvert : une conversation peut croiser deux comptes"
    );

    sqlx::query("insert into conversations (utilisateur_id, personnage_id) values (1, $1)")
        .bind(COMPAGNON_A)
        .execute(base.pool())
        .await
        .expect("la conversation légitime doit passer");
    println!("conversation de 1 vers son propre compagnon -> acceptée");

    jetable.detruire().await;
}

#[tokio::test]
async fn les_bornes_de_forme_des_parametres_sont_tenues() {
    let (jetable, base) = deux_compagnons().await;

    // Un curseur hors [0,1] fausserait la résolution des plafonds sans qu'aucun code ne s'en
    // aperçoive : `least(1.4, plafond)` rend le plafond, donc la valeur aberrante passerait
    // inaperçue tant qu'aucun plafond n'existe.
    let hors_bornes = sqlx::query(
        "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
         values ($1, 'humour', 1.40)",
    )
    .bind(COMPAGNON_A)
    .execute(base.pool())
    .await;
    println!(
        "curseur à 1,40               -> {}",
        if hors_bornes.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(hors_bornes.is_err());

    // Une fenêtre horaire à moitié renseignée n'est interprétable par personne.
    let demi_plage = sqlx::query(
        "insert into personnage_parametres_interaction (personnage_id, plage_horaire_debut)
         values ($1, '09:00')",
    )
    .bind(COMPAGNON_A)
    .execute(base.pool())
    .await;
    println!(
        "fenêtre horaire à moitié     -> {}",
        if demi_plage.is_err() {
            "REFUSÉE"
        } else {
            "acceptée"
        }
    );
    assert!(demi_plage.is_err());

    // Et une température de modèle absurde.
    let temperature = sqlx::query(
        "insert into personnage_parametres_modele
            (personnage_id, prompt_systeme_genere, prompt_systeme_hash, modele_cible, temperature)
         values ($1, 't', 'h', 'm', 9.00)",
    )
    .bind(COMPAGNON_B)
    .execute(base.pool())
    .await;
    println!(
        "température à 9,00           -> {}",
        if temperature.is_err() {
            "REFUSÉE"
        } else {
            "acceptée"
        }
    );
    assert!(temperature.is_err());

    jetable.detruire().await;
}
