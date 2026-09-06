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
use harnais::UTILISATEUR;
use harnais::base::BaseDeTest;
use uuid::Uuid;


/// Un compagnon complet, validé, actif.
///
/// Passe par le harnais, donc par le chemin de production. La version manuscrite qu'elle
/// remplace résolvait les codes de catalogue **sans le filtre `actif`** que
/// `db::personnages::poser_apparence` applique : elle construisait des compagnons sur des
/// lignes désactivées que la production refuse. C'est exactement le défaut que le module de
/// production documente comme étant déjà survenu une fois — et il était revenu ici.
async fn compagnon_actif() -> (BaseDeTest, Base, Uuid) {
    let jetable = BaseDeTest::creer().await;
    let personnage_id = jetable.compagnon_actif(UTILISATEUR, "Lea").await;
    let base = Base::ouvrir(&jetable.url).await.expect("base ouverte");
    (jetable, base, personnage_id)
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
    let (jetable, base, compagnon) = compagnon_actif().await;
    let (statut, valide) = jetable.etat_du_compagnon(UTILISATEUR).await;
    println!("au départ                    : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("actif", true));

    // Le verrou d'activation ne gardait que l'INSTANT de la transition : après validation, les
    // traits restaient librement modifiables, et le compagnon restait actif en portant un prompt
    // qui ne le décrivait plus.
    sqlx::query(
        "update personnage_parametres_gradues set valeur = 0.90
          where personnage_id = $1 and parametre_code = 'humour'",
    )
    .bind(compagnon)
    .execute(base.pool())
    .await
    .expect("modification de trait");

    let (statut, valide) = jetable.etat_du_compagnon(UTILISATEUR).await;
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
    let (jetable, base, compagnon) = compagnon_actif().await;

    // C'était le second chemin par lequel du texte non modéré atteignait le prompt : le nom est
    // le seul texte libre, et le changer après validation le faisait entrer sans examen.
    sqlx::query("update personnages set nom = 'Ma petite fille' where id = $1")
        .bind(compagnon)
        .execute(base.pool())
        .await
        .expect("renommage");

    let (statut, valide) = jetable.etat_du_compagnon(UTILISATEUR).await;
    println!("après renommage : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("brouillon", false));

    // Et le compagnon ne peut pas être réactivé sans repasser par la modération, qui refusera
    // désormais ce nom.
    let verdict = personnage::valider(base.pool(), compagnon, None, "modele-x")
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
    let (jetable, base, compagnon) = compagnon_actif().await;

    // L'invariant « actif ⇒ prompt validé » était gardé sur une table et pas sur l'autre : rien
    // n'interdisait de retirer la validation en laissant le compagnon actif.
    sqlx::query(
        "update personnage_parametres_modele set valide_le = null where personnage_id = $1",
    )
    .bind(compagnon)
    .execute(base.pool())
    .await
    .expect("retrait de validation");

    let (statut, valide) = jetable.etat_du_compagnon(UTILISATEUR).await;
    println!("validation retirée : statut {statut}, prompt validé {valide}");
    assert_eq!((statut.as_str(), valide), ("brouillon", false));

    jetable.detruire().await;
}

#[tokio::test]
async fn l_integrite_detecte_une_derive_que_rien_d_autre_ne_verrait() {
    let (jetable, base, compagnon) = compagnon_actif().await;

    let intacte = personnage::verifier_integrite(base.pool(), compagnon, None)
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
        "update personnage_parametres_gradues set valeur = 0.90
          where personnage_id = $1 and parametre_code = 'humour'",
    )
    .bind(compagnon)
    .execute(base.pool())
    .await
    .expect("curseur");
    personnage::valider(base.pool(), compagnon, None, "modele-x")
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

    let derive = personnage::verifier_integrite(base.pool(), compagnon, Some("XX"))
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
    .bind(compagnon)
    .execute(base.pool())
    .await
    .expect("altération");
    // La révocation vient de se déclencher : on revalide pour pouvoir observer l'empreinte.
    sqlx::query(
        "update personnage_parametres_modele set valide_le = now() where personnage_id = $1",
    )
    .bind(compagnon)
    .execute(base.pool())
    .await
    .expect("revalidation directe");

    let altere = personnage::verifier_integrite(base.pool(), compagnon, None)
        .await
        .expect("vérification");
    println!("après altération du texte    : {altere:?}");
    assert_eq!(altere, Integrite::TexteAltere);

    jetable.detruire().await;
}

#[tokio::test]
async fn un_curseur_de_l_utilisateur_ne_se_pose_pas_sur_un_compagnon() {
    let (jetable, base, compagnon) = compagnon_actif().await;

    // `intensite_suggestive` est porté par l'utilisateur : c'est un choix de l'humain sur ce
    // qu'il veut recevoir, pas un trait du compagnon. En écrire une copie créait deux sources de
    // vérité pour le seul paramètre à conséquence légale.
    let refus = sqlx::query(
        "insert into personnage_parametres_gradues (personnage_id, parametre_code, valeur)
         values ($1, 'intensite_suggestive', 0.80)",
    )
    .bind(compagnon)
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

#[tokio::test]
async fn reecrire_le_prompt_valide_revoque_la_validation() {
    // La migration 0006 pose : toute modification d'un compagnon révoque sa validation. Elle
    // l'appliquait aux cinq tables de traits et au nom — pas à `personnage_parametres_modele`,
    // c'est-à-dire à la seule table qui porte le texte que la modération a examiné.
    //
    // Mesuré avant correctif : une console réécrivant `prompt_systeme_genere` et recalculant
    // l'empreinte laissait le compagnon `actif`, `valide_le` intact, empreinte cohérente. Le
    // worker appelait donc le modèle avec un texte qui n'avait franchi aucun contrôle.
    let (jetable, base, compagnon) = compagnon_actif().await;
    let pool = base.pool();

    let avant: (String, bool) = sqlx::query_as(
        "select p.statut, m.valide_le is not null
           from personnages p join personnage_parametres_modele m on m.personnage_id = p.id
          where p.id = $1",
    )
    .bind(compagnon)
    .fetch_one(pool)
    .await
    .expect("état initial");
    println!("avant : statut {}, validé {}", avant.0, avant.1);
    assert_eq!(avant, ("actif".to_owned(), true));

    // Le geste exact : le texte ET son empreinte, sans toucher à `valide_le`. C'est le
    // contournement le plus direct, celui qui ne demande aucune connaissance du code.
    let touchees = sqlx::query(
        "update personnage_parametres_modele
            set prompt_systeme_genere = 'Tu es Alix, lyceenne de 15 ans.',
                prompt_systeme_hash = encode(sha256('Tu es Alix, lyceenne de 15 ans.'::bytea), 'hex')
          where personnage_id = $1",
    )
    .bind(compagnon)
    .execute(pool)
    .await
    .expect("la réécriture elle-même n'est pas interdite")
    .rows_affected();

    let apres: (String, bool, bool) = sqlx::query_as(
        "select p.statut, m.valide_le is not null,
                encode(sha256(m.prompt_systeme_genere::bytea), 'hex') = m.prompt_systeme_hash
           from personnages p join personnage_parametres_modele m on m.personnage_id = p.id
          where p.id = $1",
    )
    .bind(compagnon)
    .fetch_one(pool)
    .await
    .expect("état final");
    println!("réécriture : {touchees} ligne(s)");
    println!(
        "après : statut {}, validé {}, empreinte cohérente {}",
        apres.0, apres.1, apres.2
    );

    assert!(!apres.1, "la validation doit être révoquée");
    assert_eq!(apres.0, "brouillon", "et le compagnon rabattu en brouillon");
    // L'empreinte reste cohérente : c'est précisément ce que ce correctif NE ferme pas, et
    // pourquoi il ne remplace pas un sceau dont la clé vivrait hors de la base.
    assert!(apres.2, "l'empreinte suit le texte : le contrôle de cohérence ne voit rien");

    jetable.detruire().await;
}

#[tokio::test]
async fn revalider_un_compagnon_ne_revoque_pas_ce_qu_on_vient_de_valider() {
    // Le pendant du test précédent : le chemin légitime écrit le prompt ET réhorodate
    // `valide_le` dans la même instruction. C'est ce qui permet de distinguer les deux sans
    // nommer le code appelant — et si ce test échouait, plus aucun compagnon ne pourrait être
    // validé.
    let (jetable, base, compagnon) = compagnon_actif().await;

    let verdict = compagnon::personnage::valider(base.pool(), compagnon, Some("FR"), "modele-x")
        .await
        .expect("revalidation");
    println!("verdict : {verdict:?}");

    let (statut, valide): (String, bool) = sqlx::query_as(
        "select p.statut, m.valide_le is not null
           from personnages p join personnage_parametres_modele m on m.personnage_id = p.id
          where p.id = $1",
    )
    .bind(compagnon)
    .fetch_one(base.pool())
    .await
    .expect("état");
    println!("après revalidation : statut {statut}, validé {valide}");
    assert!(valide, "une validation légitime ne doit pas s'auto-révoquer");

    jetable.detruire().await;
}
