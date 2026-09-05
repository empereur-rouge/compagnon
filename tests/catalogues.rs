//! Les vocabulaires contrôlés : ce qu'ils contiennent, et ce qu'ils ne peuvent pas contenir.
//!
//! # Ce que ces tests protègent
//!
//! Le prompt système n'est jamais saisi : il est composé à partir de ces catalogues. La sûreté
//! du produit repose donc entièrement sur l'ensemble des valeurs possibles, pas sur un filtre
//! appliqué après coup. Ces tests éprouvent cet ensemble — et surtout ce que la base **refuse**
//! d'y laisser entrer.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use compagnon::db::Base;
use compagnon::db::catalogues::{self, Catalogue};
use harnais::base::BaseDeTest;
use rust_decimal::Decimal;

/// Ouvre la base de test et rend le pool, avec de quoi la détruire.
async fn base() -> (BaseDeTest, Base) {
    let jetable = BaseDeTest::creer().await;
    let base = Base::ouvrir(&jetable.url).await.expect("base migrée");
    (jetable, base)
}

#[tokio::test]
async fn aucune_tranche_d_age_ne_peut_evoquer_un_mineur() {
    let (jetable, base) = base().await;

    // Ce que le catalogue contient.
    let tranches = catalogues::tranches_age(base.pool())
        .await
        .expect("tranches lisibles");
    for tranche in &tranches {
        println!(
            "{:10} {:20} âge plancher : {}",
            tranche.code, tranche.libelle, tranche.age_min
        );
        assert!(
            tranche.age_min >= 25,
            "la tranche « {} » descend à {} ans",
            tranche.code,
            tranche.age_min
        );
    }
    assert!(
        !tranches.is_empty(),
        "le catalogue doit proposer des tranches"
    );

    // Et surtout : ce qu'il ne PEUT pas contenir. Un catalogue correct aujourd'hui ne prouve
    // rien ; ce qui protège, c'est que la base refuse la ligne, quel que soit le chemin —
    // code Rust, console psql, restauration partielle.
    for (libelle, age) in [
        ("Adolescente", 16_i16),
        ("Jeune, 18 ans", 18),
        ("Presque 25", 24),
    ] {
        let refus = sqlx::query(
            "insert into ref_tranches_age_apparent (code, libelle, age_min) values ($1, $2, $3)",
        )
        .bind(format!("essai_{age}"))
        .bind(libelle)
        .bind(age)
        .execute(base.pool())
        .await;
        println!(
            "insertion « {libelle} » ({age} ans) -> {}",
            if refus.is_err() {
                "REFUSÉE"
            } else {
                "acceptée"
            }
        );
        assert!(
            refus.is_err(),
            "la base a accepté une tranche d'âge à {age} ans"
        );
    }
    println!("\nle plancher de 25 ans est tenu par la base, pas par une convention");
    jetable.detruire().await;
}

#[tokio::test]
async fn une_base_neuve_permet_de_composer_un_compagnon() {
    let (jetable, base) = base().await;

    // Le peuplement vit dans la migration : ces valeurs ne sont pas des données d'exemple, ce
    // sont des constantes du produit. Une base migrée sans elles laisserait le service
    // incapable de créer quoi que ce soit — panne qui ne se déclarerait qu'au premier
    // utilisateur.
    for catalogue in Catalogue::tous() {
        let options = catalogues::lister(base.pool(), catalogue)
            .await
            .expect("catalogue lisible");
        let apercu: Vec<&str> = options.iter().take(3).map(|o| o.libelle.as_str()).collect();
        println!(
            "{catalogue:20?} {:>2} options : {}…",
            options.len(),
            apercu.join(", ")
        );
        assert!(!options.is_empty(), "{catalogue:?} est vide");
    }

    let archetypes = catalogues::archetypes(base.pool())
        .await
        .expect("archétypes");
    let tons = catalogues::tons(base.pool()).await.expect("tons");
    let curseurs = catalogues::parametres_gradues(base.pool())
        .await
        .expect("curseurs");
    println!(
        "\n{} archétypes, {} tons, {} curseurs",
        archetypes.len(),
        tons.len(),
        curseurs.len()
    );
    assert!(
        archetypes.len() >= 10,
        "trop peu d'archétypes pour composer"
    );
    assert!(tons.len() >= 8, "trop peu de tons");

    // Chaque description est reprise telle quelle dans le prompt : une vide y ferait un trou.
    for trait_ in archetypes.iter().chain(tons.iter()) {
        assert!(
            !trait_.description.trim().is_empty(),
            "« {} » n'a pas de description, elle manquerait au prompt",
            trait_.code
        );
    }
    println!("toutes les descriptions sont renseignées");
    jetable.detruire().await;
}

#[tokio::test]
async fn une_fusion_est_orientee_et_ne_repond_pas_a_l_envers() {
    let (jetable, base) = base().await;

    // « Principalement timide avec une pointe de dominance » n'est pas « principalement dominant
    // avec une pointe de timidité ». Le yandere, c'est le premier — et si la table répondait
    // dans les deux sens, la composition donnerait le mauvais personnage à qui a choisi
    // l'inverse.
    let yandere = catalogues::fusion_archetypes(base.pool(), "timide", "dominant")
        .await
        .expect("lecture");
    println!(
        "timide + dominant -> {:?}",
        yandere.as_ref().map(|f| &f.nom_fusion)
    );
    let yandere = yandere.expect("cette fusion est au catalogue");
    assert_eq!(yandere.nom_fusion, "Yandere");
    println!("  description : {}", yandere.description_fusion);

    let inverse = catalogues::fusion_archetypes(base.pool(), "dominant", "timide")
        .await
        .expect("lecture");
    println!(
        "dominant + timide -> {:?}",
        inverse.as_ref().map(|f| &f.nom_fusion)
    );
    assert!(
        inverse.is_none(),
        "la fusion a répondu à l'envers : l'orientation ne sert à rien"
    );

    // Une combinaison non répertoriée n'est pas une erreur : la composition additionne alors
    // les deux descriptions simples.
    let inconnue = catalogues::fusion_archetypes(base.pool(), "calme", "loyal")
        .await
        .expect("lecture");
    println!(
        "calme + loyal    -> {:?} (combinaison libre, pas une erreur)",
        inconnue.is_none()
    );
    assert!(inconnue.is_none());

    jetable.detruire().await;
}

#[tokio::test]
async fn un_plafond_de_juridiction_abaisse_la_valeur_effective() {
    let (jetable, base) = base().await;

    let curseurs = catalogues::parametres_gradues(base.pool())
        .await
        .expect("curseurs");
    let suggestif = curseurs
        .iter()
        .find(|c| c.code == "intensite_suggestive")
        .expect("le curseur existe");
    println!(
        "{:22} défaut {} plafonnable : {}",
        suggestif.code, suggestif.valeur_defaut, suggestif.plafonnable_juridiction
    );
    assert!(
        suggestif.plafonnable_juridiction,
        "l'intensité suggestive est le seul curseur qui doive se plafonner par pays"
    );
    assert_eq!(
        suggestif.valeur_defaut,
        Decimal::ZERO,
        "le défaut doit être zéro : rien de suggestif sans choix explicite"
    );

    // Aucun plafond tant que le pays n'a pas été examiné.
    let avant = catalogues::plafond(base.pool(), "FR", "intensite_suggestive")
        .await
        .expect("lecture");
    println!("plafond FR avant revue légale : {avant:?}");
    assert!(avant.is_none());

    sqlx::query(
        "insert into ref_plafonds_juridiction (code_pays, parametre_code, valeur_max, source_legale)
         values ('FR', 'intensite_suggestive', 0.40, 'revue interne, essai')",
    )
    .execute(base.pool())
    .await
    .expect("plafond posé");

    let apres = catalogues::plafond(base.pool(), "FR", "intensite_suggestive")
        .await
        .expect("lecture");
    println!("plafond FR après revue légale : {apres:?}");
    assert_eq!(apres, Some(Decimal::new(40, 2)));

    // La valeur effective est le minimum des deux — c'est ce que la composition appliquera.
    let choisie = Decimal::new(90, 2);
    let effective = apres.map_or(choisie, |plafond| choisie.min(plafond));
    println!("choisie {choisie} plafonnée à {apres:?} -> effective {effective}");
    assert_eq!(effective, Decimal::new(40, 2));

    jetable.detruire().await;
}
