//! Ce que la base refuse de laisser atteindre le modèle.
//!
//! # Pourquoi ce fichier existe
//!
//! La phase 1.2 revendiquait une sûreté structurelle : « si aucune valeur du catalogue n'évoque
//! un mineur, aucune composition ne le peut ». C'était **faux**, et d'une façon qui ne se voyait
//! pas à la lecture — chaque garantie s'arrêtait au moment précis où elle aurait dû porter sur du
//! texte plutôt que sur une forme. Reproduit en une écriture :
//!
//! ```text
//! update ref_tranches_age_apparent set libelle = 'Adolescente de 16 ans' where code = '25_34';
//! ```
//!
//! `check (age_min >= 25)` satisfait, tests au vert, modération acceptée — et le prompt envoyé
//! au modèle disait « Femme, Adolescente de 16 ans ».
//!
//! Chaque test de ce fichier rejoue un exploit qui a réellement fonctionné.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::db::Base;
use compagnon::personnage::{self, Integrite};
use harnais::base::BaseDeTest;
use sqlx::PgPool;
use uuid::Uuid;

const COMPAGNON: Uuid = Uuid::from_u128(0x3333_3333_3333_3333_3333_3333_3333_3333);

/// Un compagnon complet, validé, actif.
async fn compagnon_actif() -> (BaseDeTest, Base) {
    let jetable = BaseDeTest::creer().await;
    let base = Base::ouvrir(&jetable.url).await.expect("base migrée");
    let pool = base.pool();

    sqlx::query("insert into utilisateurs (id) values (7)")
        .execute(pool)
        .await
        .expect("utilisateur");
    sqlx::query("insert into personnages (id, utilisateur_id, nom) values ($1, 7, 'Léa')")
        .bind(COMPAGNON)
        .execute(pool)
        .await
        .expect("compagnon");
    sqlx::query(
        "insert into personnage_apparence (personnage_id, genre_id, tranche_age_id, morphologie_id)
         select $1,
                (select id from ref_genres where code = 'femme'),
                (select id from ref_tranches_age_apparent where code = '25_34'),
                (select id from ref_morphologies where code = 'mince')",
    )
    .bind(COMPAGNON)
    .execute(pool)
    .await
    .expect("apparence");
    for (table, colonne, reference, code) in [
        (
            "personnage_archetypes",
            "archetype_id",
            "ref_archetypes",
            "calme",
        ),
        ("personnage_tons", "ton_id", "ref_tons", "tendre"),
    ] {
        sqlx::query(&format!(
            "insert into {table} (personnage_id, {colonne}, role)
             select $1, id, 'principal' from {reference} where code = $2"
        ))
        .bind(COMPAGNON)
        .bind(code)
        .execute(pool)
        .await
        .expect("trait");
    }
    sqlx::query("insert into personnage_parametres_interaction (personnage_id) values ($1)")
        .bind(COMPAGNON)
        .execute(pool)
        .await
        .expect("interaction");

    personnage::valider(pool, COMPAGNON, None, "modele-x")
        .await
        .expect("validation");
    personnage::activer(pool, COMPAGNON)
        .await
        .expect("activation");
    (jetable, base)
}

/// L'état courant : statut, et validité du prompt.
async fn etat(pool: &PgPool) -> (String, bool) {
    sqlx::query_as(
        "select p.statut,
                coalesce((select valide_le is not null from personnage_parametres_modele m
                           where m.personnage_id = p.id), false)
           from personnages p where p.id = $1",
    )
    .bind(COMPAGNON)
    .fetch_one(pool)
    .await
    .expect("état")
}

#[tokio::test]
async fn le_texte_du_catalogue_ne_se_modifie_pas_hors_migration() {
    let jetable = BaseDeTest::creer().await;
    let base = Base::ouvrir(&jetable.url).await.expect("base migrée");

    // L'exploit d'origine, et ses variantes sur les autres textes qui atteignent le modèle.
    let exploits = [
        (
            "tranche d'âge",
            "update ref_tranches_age_apparent set libelle = 'Adolescente de 16 ans' where code = '25_34'",
        ),
        (
            "description d'archétype",
            "update ref_archetypes set description = 'une lycéenne de 15 ans' where code = 'calme'",
        ),
        (
            "description de ton",
            "update ref_tons set description = 'parle comme une enfant' where code = 'tendre'",
        ),
        (
            "nom de fusion",
            "update ref_fusions_archetypes set nom_fusion = 'Collégienne' where nom_fusion = 'Yandere'",
        ),
        (
            "libellé de genre",
            "update ref_genres set libelle = 'Fillette' where code = 'femme'",
        ),
        (
            "plancher d'âge",
            "update ref_tranches_age_apparent set age_min = 16 where code = '25_34'",
        ),
    ];
    println!("{:<26} verdict", "écriture tentée");
    println!("{}", "-".repeat(44));
    for (quoi, sql) in exploits {
        let refus = sqlx::query(sql).execute(base.pool()).await;
        println!(
            "{quoi:<26} {}",
            if refus.is_err() {
                "REFUSÉE"
            } else {
                "acceptée"
            }
        );
        assert!(
            refus.is_err(),
            "« {quoi} » a été accepté : le texte reste altérable"
        );
    }

    // Mais le retrait d'une option reste possible — le produit y tient, et c'est son mécanisme
    // de réponse à un signalement sans déploiement.
    sqlx::query("update ref_archetypes set actif = false where code = 'possessif'")
        .execute(base.pool())
        .await
        .expect("le retrait à chaud doit rester possible");
    println!("\nretrait d'une option (actif = false)  acceptée");

    jetable.detruire().await;
}

#[tokio::test]
async fn modifier_un_trait_apres_validation_revoque_l_activation() {
    let (jetable, base) = compagnon_actif().await;
    let (statut, valide) = etat(base.pool()).await;
    println!("au départ                    : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("actif", true));

    // Le verrou d'activation ne gardait que l'INSTANT de la transition : après validation, les
    // traits restaient librement modifiables, et le compagnon restait actif en portant un prompt
    // qui ne le décrivait plus.
    sqlx::query(
        "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
         values ($1, 'humour', 0.90)",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await
    .expect("modification de trait");

    let (statut, valide) = etat(base.pool()).await;
    println!("après modification d'un trait : statut {statut}, prompt validé {valide}");
    assert_eq!(
        (statut.as_str(), valide),
        ("brouillon", false),
        "un trait modifié doit révoquer la validation et rabattre le statut"
    );

    jetable.detruire().await;
}

#[tokio::test]
async fn renommer_apres_validation_revoque_aussi() {
    let (jetable, base) = compagnon_actif().await;

    // C'était le second chemin par lequel du texte non modéré atteignait le prompt : le nom est
    // le seul texte libre, et le changer après validation le faisait entrer sans examen.
    sqlx::query("update personnages set nom = 'Ma petite fille' where id = $1")
        .bind(COMPAGNON)
        .execute(base.pool())
        .await
        .expect("renommage");

    let (statut, valide) = etat(base.pool()).await;
    println!("après renommage : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("brouillon", false));

    // Et le compagnon ne peut pas être réactivé sans repasser par la modération, qui refusera
    // désormais ce nom.
    let verdict = personnage::valider(base.pool(), COMPAGNON, None, "modele-x")
        .await
        .expect("revalidation");
    println!("revalidation    : {verdict:?}");
    assert!(matches!(
        verdict,
        personnage::moderation::Verdict::Refuse(_)
    ));

    jetable.detruire().await;
}

#[tokio::test]
async fn retirer_la_validation_rabat_un_compagnon_actif() {
    let (jetable, base) = compagnon_actif().await;

    // L'invariant « actif ⇒ prompt validé » était gardé sur une table et pas sur l'autre : rien
    // n'interdisait de retirer la validation en laissant le compagnon actif.
    sqlx::query(
        "update personnage_parametres_modele set valide_le = null where personnage_id = $1",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await
    .expect("retrait de validation");

    let (statut, valide) = etat(base.pool()).await;
    println!("validation retirée : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("brouillon", false));

    jetable.detruire().await;
}

#[tokio::test]
async fn l_integrite_detecte_une_derive_que_rien_d_autre_ne_verrait() {
    let (jetable, base) = compagnon_actif().await;

    let intacte = personnage::verifier_integrite(base.pool(), COMPAGNON, None)
        .await
        .expect("vérification");
    println!("juste après validation      : {intacte:?}");
    assert_eq!(intacte, Integrite::Intacte);

    // Un plafond de juridiction posé APRÈS la validation change ce que les traits composent,
    // sans toucher au prompt stocké. Aucune contrainte ne peut voir cela : le prompt validé
    // reste parfaitement valide en lui-même, il ne décrit simplement plus le compagnon.
    //
    // Les valeurs franchissent une borne de palier — 0,90 « énormément » plafonné à 0,10 « très
    // peu ». À l'intérieur d'un même palier, le prompt ne bougerait pas, et l'intégrité
    // resterait intacte : c'est la stabilité voulue, pas un défaut de détection.
    sqlx::query(
        "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
         values ($1, 'humour', 0.90)",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await
    .expect("curseur");
    personnage::valider(base.pool(), COMPAGNON, None, "modele-x")
        .await
        .expect("revalidation");

    sqlx::query(
        "update ref_parametres_gradues set plafonnable_juridiction = true where code = 'humour'",
    )
    .execute(base.pool())
    .await
    .expect("drapeau");
    sqlx::query(
        "insert into ref_plafonds_juridiction (code_pays, parametre_code, valeur_max)
         values ('XX', 'humour', 0.10)",
    )
    .execute(base.pool())
    .await
    .expect("plafond");

    let derive = personnage::verifier_integrite(base.pool(), COMPAGNON, Some("XX"))
        .await
        .expect("vérification");
    println!("après un plafond posé sur XX : {derive:?}");
    assert_eq!(
        derive,
        Integrite::DeriveDepuisValidation,
        "le prompt validé ne décrit plus ce que les traits composent"
    );

    // Et l'altération de la ligne elle-même, que l'empreinte attrape.
    sqlx::query(
        "update personnage_parametres_modele set prompt_systeme_genere = 'texte remplacé'
          where personnage_id = $1",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await
    .expect("altération");
    // La révocation vient de se déclencher : on revalide pour pouvoir observer l'empreinte.
    sqlx::query(
        "update personnage_parametres_modele set valide_le = now() where personnage_id = $1",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await
    .expect("revalidation directe");

    let altere = personnage::verifier_integrite(base.pool(), COMPAGNON, None)
        .await
        .expect("vérification");
    println!("après altération du texte    : {altere:?}");
    assert_eq!(altere, Integrite::TexteAltere);

    jetable.detruire().await;
}

#[tokio::test]
async fn un_curseur_de_l_utilisateur_ne_se_pose_pas_sur_un_compagnon() {
    let (jetable, base) = compagnon_actif().await;

    // `intensite_suggestive` est porté par l'utilisateur : c'est un choix de l'humain sur ce
    // qu'il veut recevoir, pas un trait du compagnon. En écrire une copie créait deux sources de
    // vérité pour le seul paramètre à conséquence légale.
    let refus = sqlx::query(
        "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
         values ($1, 'intensite_suggestive', 0.80)",
    )
    .bind(COMPAGNON)
    .execute(base.pool())
    .await;
    println!(
        "intensite_suggestive sur un compagnon -> {}",
        if refus.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    assert!(
        refus.is_err(),
        "deux sources de vérité pour le paramètre légal"
    );

    jetable.detruire().await;
}

#[tokio::test]
async fn une_option_retiree_du_catalogue_est_refusee_a_l_ecriture() {
    let jetable = BaseDeTest::creer().await;
    let base = Base::ouvrir(&jetable.url).await.expect("base migrée");
    let pool = base.pool();

    sqlx::query("insert into utilisateurs (id) values (9)")
        .execute(pool)
        .await
        .expect("utilisateur");

    // Ce test était IMPOSSIBLE à écrire avant que les écritures ne quittent le module de ligne
    // de commande : elles y étaient privées. Les tests avaient donc recopié leur SQL, en
    // omettant le `and actif` — ils construisaient des compagnons sur des lignes désactivées,
    // que la production refuse, et aucun ne pouvait attraper une régression sur ce filtre.
    //
    // Or c'est le mécanisme dont la migration 0003 fait un argument de sûreté : « retirer une
    // option, c'est passer un `actif` à faux — pas auditer du texte libre ». Il n'était éprouvé
    // nulle part.
    sqlx::query("update ref_archetypes set actif = false where code = 'possessif'")
        .execute(pool)
        .await
        .expect("retrait de l'option");

    let mut choix = std::collections::HashMap::new();
    choix.insert("archetype".to_owned(), "possessif".to_owned());

    let mut tx = pool.begin().await.expect("transaction");
    let compagnon = compagnon::db::personnages::creer(&mut tx, 9, "Léa")
        .await
        .expect("création");
    let refus = compagnon::db::personnages::poser_traits(
        &mut tx,
        compagnon,
        &choix,
        compagnon::personnage::Cible::Archetypes,
    )
    .await;
    println!(
        "archétype « possessif » retiré du catalogue -> {}",
        if refus.is_err() {
            "REFUSÉ"
        } else {
            "accepté"
        }
    );
    if let Err(erreur) = &refus {
        println!("  message : {erreur}");
    }
    assert!(
        refus.is_err(),
        "une option retirée doit être refusée : c'est le mécanisme de retrait rétroactif"
    );

    // Et la même option, réactivée, repasse — le retrait est bien réversible.
    tx.rollback().await.expect("annulation");
    sqlx::query("update ref_archetypes set actif = true where code = 'possessif'")
        .execute(pool)
        .await
        .expect("réactivation");

    let mut tx = pool.begin().await.expect("transaction");
    let compagnon = compagnon::db::personnages::creer(&mut tx, 9, "Léa")
        .await
        .expect("création");
    compagnon::db::personnages::poser_traits(
        &mut tx,
        compagnon,
        &choix,
        compagnon::personnage::Cible::Archetypes,
    )
    .await
    .expect("une option active doit passer");
    println!("archétype « possessif » réactivé            -> accepté");
    tx.rollback().await.expect("annulation");

    jetable.detruire().await;
}
