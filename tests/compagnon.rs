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
use std::collections::HashMap;

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

/// Pose un archétype sur un compagnon, par le SQL brut.
///
/// Volontairement direct : ces tests éprouvent ce que la BASE refuse — un second principal, un
/// troisième secondaire, un rang incohérent — donc ils doivent pouvoir soumettre des formes que
/// la production ne construirait jamais. Les fabriques de compagnon complet, elles, passent par
/// `db::personnages` (voir `compagnon_complet`).
async fn poser_archetype(
    pool: &PgPool,
    compagnon: Uuid,
    code: &str,
    role: &str,
    rang: Option<i16>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into personnage_archetypes (personnage_id, archetype_id, role, rang)
         select $1, id, $3, $4 from ref_archetypes where code = $2 and actif",
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

/// Compose un compagnon complet **par le chemin de production**.
///
/// Appelle `db::personnages`, comme la ligne de commande et comme le fera l'inscription depuis
/// Telegram. La version précédente réécrivait ce SQL à la main et avait déjà divergé : elle
/// omettait le `and actif`, donc construisait des compagnons sur des lignes de catalogue
/// désactivées que la production refuse — et aucun test ne pouvait attraper une régression sur
/// ce filtre.
async fn compagnon_complet(pool: &PgPool, id: Uuid, curseurs: &[(&str, &str)]) {
    use compagnon::db::personnages;
    use compagnon::personnage::Cible;

    let mut choix: HashMap<String, String> = [
        ("genre", "femme"),
        ("age", "25_34"),
        ("morphologie", "elancee"),
        ("cheveux", "brun"),
        ("longueur_cheveux", "mi_longs"),
        ("yeux", "vert"),
        ("style", "decontracte"),
        ("archetype", "timide"),
        ("archetype2", "dominant"),
        ("ton", "tendre"),
    ]
    .iter()
    .map(|(c, v)| ((*c).to_owned(), (*v).to_owned()))
    .collect();
    for (code, valeur) in curseurs {
        choix.insert((*code).to_owned(), (*valeur).to_owned());
    }

    let mut tx = pool.begin().await.expect("transaction");
    personnages::poser_apparence(&mut tx, id, &choix)
        .await
        .expect("apparence");
    personnages::poser_traits(&mut tx, id, &choix, Cible::Archetypes)
        .await
        .expect("archétypes");
    personnages::poser_traits(&mut tx, id, &choix, Cible::Tons)
        .await
        .expect("tons");
    personnages::poser_curseurs(&mut tx, id, &choix)
        .await
        .expect("curseurs");
    sqlx::query("insert into personnage_parametres_interaction (personnage_id) values ($1)")
        .bind(id)
        .execute(&mut *tx)
        .await
        .expect("interaction");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn un_compagnon_en_base_compose_son_prompt_avec_sa_fusion() {
    let (jetable, base) = deux_compagnons().await;
    compagnon_complet(
        base.pool(),
        COMPAGNON_A,
        &[
            ("humour", "0.70"),
            ("affection", "0.90"),
            ("assurance", "0.30"),
        ],
    )
    .await;

    let traits = compagnon::personnage::charger(base.pool(), COMPAGNON_A, Some("FR"))
        .await
        .expect("chargement");
    let prompt = compagnon::personnage::composer(&traits);
    println!("=============== PROMPT COMPOSÉ ===============");
    println!("{}", prompt.texte);

    // La fusion timide + dominant est au catalogue : elle doit avoir été résolue à la lecture.
    assert!(
        prompt.texte.contains("Yandere"),
        "la fusion du catalogue n'a pas été résolue"
    );
    assert!(
        !prompt.texte.contains("réservé au premier abord"),
        "la fusion doit remplacer les descriptions simples"
    );

    // L'âge vient du NOMBRE contraint, jamais du libellé de la tranche. Le libellé était ce qui
    // atteignait le modèle et rien ne le contraignait : une seule écriture suffisait à faire
    // dire au prompt « Adolescente de 16 ans » avec `age_min` resté à 25.
    println!(
        "\nligne d'apparence : {}",
        extraire_ligne(&prompt.texte, "apparence d'au moins")
    );
    assert!(
        prompt.texte.contains("apparence d'au moins 25 ans"),
        "l'âge doit venir de age_min, pas du libellé"
    );

    jetable.detruire().await;
}

#[tokio::test]
async fn un_plafond_ne_s_applique_qu_aux_curseurs_declares_plafonnables() {
    let (jetable, base) = deux_compagnons().await;
    compagnon_complet(base.pool(), COMPAGNON_A, &[("affection", "0.90")]).await;

    // C'est la règle que la version précédente de ce test avait à l'envers : elle posait le
    // plafond sur `affection`, constatait qu'il s'appliquait, et prenait cela pour le
    // comportement voulu. Or la jointure filtrait sur le DOMAINE et non sur le drapeau — donc
    // les plafonds ne pouvaient s'appliquer qu'à des paramètres déclarés NON plafonnables, et
    // jamais à celui pour lequel le mécanisme légal existe.
    let plafonnables: Vec<(String, bool, bool)> = sqlx::query_as(
        "select code, plafonnable_juridiction, entre_dans_le_prompt
           from ref_parametres_gradues order by code",
    )
    .fetch_all(base.pool())
    .await
    .expect("catalogue");
    println!("{:<22} plafonnable  dans le prompt", "curseur");
    for (code, plafonnable, dans_le_prompt) in &plafonnables {
        println!("{code:<22} {plafonnable:<12} {dans_le_prompt}");
    }

    // Un plafond posé sur un curseur NON plafonnable est ignoré — c'est le comportement voulu :
    // le drapeau est la déclaration qu'une différence légale existe pour ce paramètre-là.
    sqlx::query(
        "insert into ref_plafonds_juridiction (code_pays, parametre_code, valeur_max, source_legale)
         values ('XX', 'affection', 0.30, 'essai — affection n''est pas plafonnable')",
    )
    .execute(base.pool())
    .await
    .expect("plafond");

    let traits = compagnon::personnage::charger(base.pool(), COMPAGNON_A, Some("XX"))
        .await
        .expect("chargement");
    let affection = traits
        .curseurs
        .iter()
        .find(|c| c.code == "affection")
        .expect("curseur chargé");
    println!(
        "\naffection : choisie 0,90, plafond XX à 0,30 posé -> effective {}",
        affection.valeur
    );
    assert_eq!(
        affection.valeur,
        rust_decimal::Decimal::new(90, 2),
        "un plafond sur un curseur non plafonnable doit être ignoré"
    );

    // Et quand le curseur EST déclaré plafonnable, le plafond mord.
    sqlx::query(
        "update ref_parametres_gradues set plafonnable_juridiction = true where code = 'affection'",
    )
    .execute(base.pool())
    .await
    .expect("drapeau");

    let traits = compagnon::personnage::charger(base.pool(), COMPAGNON_A, Some("XX"))
        .await
        .expect("chargement");
    let affection = traits
        .curseurs
        .iter()
        .find(|c| c.code == "affection")
        .expect("curseur chargé");
    println!(
        "affection, une fois déclarée plafonnable       -> effective {} (choisie {:?})",
        affection.valeur, affection.avant_plafond
    );
    assert_eq!(affection.valeur, rust_decimal::Decimal::new(30, 2));
    assert_eq!(
        affection.avant_plafond,
        Some(rust_decimal::Decimal::new(90, 2))
    );

    jetable.detruire().await;
}

/// La ligne de dosage d'un curseur, pour rendre la sortie lisible.
fn extraire_ligne(texte: &str, code: &str) -> String {
    texte
        .lines()
        .find(|l| l.contains(code))
        .unwrap_or("(absente)")
        .trim()
        .to_owned()
}

#[tokio::test]
async fn la_moderation_ouvre_ou_ferme_le_verrou_d_activation() {
    let (jetable, base) = deux_compagnons().await;
    compagnon_complet(base.pool(), COMPAGNON_A, &[("humour", "0.60")]).await;
    compagnon_complet(base.pool(), COMPAGNON_B, &[("humour", "0.60")]).await;

    // B porte un nom qui ne peut pas être retenu.
    sqlx::query("update personnages set nom = 'Ma petite fille' where id = $1")
        .bind(COMPAGNON_B)
        .execute(base.pool())
        .await
        .expect("renommage");

    // --- Le compagnon dont le nom passe ---
    let verdict = compagnon::personnage::valider(base.pool(), COMPAGNON_A, Some("FR"), "modele-x")
        .await
        .expect("validation");
    println!("compagnon « Léa »            -> {verdict:?}");
    assert_eq!(verdict, compagnon::personnage::moderation::Verdict::Accepte);

    let (prompt, empreinte, valide): (String, String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as(
            "select prompt_systeme_genere, prompt_systeme_hash, valide_le
               from personnage_parametres_modele where personnage_id = $1",
        )
        .bind(COMPAGNON_A)
        .fetch_one(base.pool())
        .await
        .expect("prompt écrit");
    println!(
        "  prompt de {} caractères, empreinte {}…",
        prompt.len(),
        &empreinte[..12]
    );
    println!("  validé le {valide:?}");
    assert!(
        valide.is_some(),
        "le prompt doit porter sa date de validation"
    );

    // Le verrou s'ouvre.
    sqlx::query("update personnages set statut = 'actif' where id = $1")
        .bind(COMPAGNON_A)
        .execute(base.pool())
        .await
        .expect("l'activation doit être permise");
    println!("  activation -> permise");

    // --- Le compagnon dont le nom ne passe pas ---
    let verdict = compagnon::personnage::valider(base.pool(), COMPAGNON_B, Some("FR"), "modele-x")
        .await
        .expect("validation");
    println!("\ncompagnon « Ma petite fille » -> {verdict:?}");
    assert!(matches!(
        verdict,
        compagnon::personnage::moderation::Verdict::Refuse(_)
    ));

    let prompts: i64 = sqlx::query_scalar(
        "select count(*) from personnage_parametres_modele where personnage_id = $1",
    )
    .bind(COMPAGNON_B)
    .fetch_one(base.pool())
    .await
    .expect("comptage");
    println!("  prompts écrits : {prompts} (aucun ne doit l'être)");
    assert_eq!(
        prompts, 0,
        "un compagnon refusé ne doit rien conserver d'activable"
    );

    let statut: String = sqlx::query_scalar("select statut from personnages where id = $1")
        .bind(COMPAGNON_B)
        .fetch_one(base.pool())
        .await
        .expect("lecture");
    println!("  statut -> {statut}");
    assert_eq!(statut, "rejete");

    let activation = sqlx::query("update personnages set statut = 'actif' where id = $1")
        .bind(COMPAGNON_B)
        .execute(base.pool())
        .await;
    println!(
        "  activation -> {}",
        if activation.is_err() {
            "REFUSÉE"
        } else {
            "permise"
        }
    );
    assert!(
        activation.is_err(),
        "un compagnon refusé ne doit pas pouvoir s'activer"
    );

    // --- L'historique raconte les deux ---
    let versions: Vec<(String, i32)> = sqlx::query_as(
        "select raison, version from personnage_historique_versions order by modifie_le",
    )
    .fetch_all(base.pool())
    .await
    .expect("historique");
    println!("\nhistorique :");
    for (raison, version) in &versions {
        println!("  v{version} : {raison}");
    }
    assert_eq!(
        versions.len(),
        2,
        "chaque décision doit laisser une version"
    );
    assert!(versions.iter().any(|(r, _)| r == "moderation_validation"));
    assert!(
        versions.iter().any(|(r, _)| r == "moderation_rejet"),
        "un refus se raconte aussi"
    );

    // L'instantané contient bien tout le compagnon.
    let etat: serde_json::Value = sqlx::query_scalar(
        "select etat_complet from personnage_historique_versions
          where personnage_id = $1 order by version desc limit 1",
    )
    .bind(COMPAGNON_A)
    .fetch_one(base.pool())
    .await
    .expect("instantané");
    let cles: Vec<&String> = etat.as_object().expect("objet").keys().collect();
    println!("\ninstantané : {cles:?}");
    for attendu in [
        "personnage",
        "apparence",
        "archetypes",
        "tons",
        "curseurs",
        "interaction",
        "modele",
    ] {
        assert!(
            etat.get(attendu).is_some(),
            "l'instantané n'a pas de « {attendu} »"
        );
    }

    jetable.detruire().await;
}
