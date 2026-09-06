//! Le registre des coûts, éprouvé sur un vrai PostgreSQL.
//!
//! Trois propriétés, dont deux qu'aucun test unitaire ne pourrait tenir :
//!
//! 1. le montant écrit est **exactement** celui relu — c'est la raison d'être du `numeric(10,6)`
//!    et du `Decimal` qui lui répond côté Rust ;
//! 2. la ligne est **immuable**, sauf l'anonymisation exigée par une purge RGPD, qui détache
//!    sans effacer le montant ;
//! 3. le vocabulaire Rust et les `check` de la base **disent la même chose** — une variante
//!    ajoutée d'un côté sans l'autre est le défaut classique de deux couches qui se mirroitent,
//!    et il ne se voit qu'à l'écriture.

#![allow(clippy::expect_used, clippy::panic)]

mod harnais;

use chrono::{Duration as DureeChrono, Utc};
use compagnon::db::consommation::{self, Appel, Origine, Statut, TypeAppel};
use compagnon::db::utilisateurs;
use harnais::base::BaseDeTest;
use rust_decimal::Decimal;

/// L'utilisateur auquel les coûts sont imputés.
const UTILISATEUR: i64 = 770_001;

/// Un appel type, dont seuls le coût et le vocabulaire varient d'un test à l'autre.
fn appel(cout_eur: Decimal) -> Appel<'static> {
    Appel {
        utilisateur_id: UTILISATEUR,
        conversation_id: None,
        message_id: None,
        type_appel: TypeAppel::Message,
        origine: Origine::Reponse,
        fournisseur: "runpod",
        modele: "mistral-small-3.2-24b",
        unites_entree: Some(812),
        unites_sortie: Some(143),
        cout_eur,
        duree: Some(std::time::Duration::from_millis(1840)),
        statut: Statut::Ok,
    }
}

/// Une base migrée, avec l'utilisateur déjà inscrit.
async fn base_prete() -> BaseDeTest {
    let base = BaseDeTest::creer().await;
    utilisateurs::assurer(base.pool(), UTILISATEUR, Some("Erwan"))
        .await
        .expect("utilisateur inscrit");
    base
}

#[tokio::test]
async fn un_cout_au_millionieme_d_euro_se_relit_exact() {
    let base = base_prete().await;

    // 0,000247 € : l'ordre de grandeur réel d'un message chez un hébergeur serverless. En
    // virgule flottante, la somme d'un million de lignes de cette taille dérive — et c'est
    // exactement la somme qu'on vient chercher dans cette table.
    let attendu = Decimal::new(247, 6);
    let id = consommation::inscrire(base.pool(), &appel(attendu))
        .await
        .expect("ligne inscrite");

    let (relu, duree_ms, modele): (Decimal, i32, String) = sqlx::query_as(
        "select cout_fournisseur_eur, duree_ms, modele from consommation where id = $1",
    )
    .bind(id)
    .fetch_one(base.pool())
    .await
    .expect("ligne relue");

    println!("écrit : {attendu} € — relu : {relu} €");
    println!("durée : {duree_ms} ms — modèle : {modele}");
    assert_eq!(relu, attendu, "le montant doit traverser la base sans dériver");
    assert_eq!(duree_ms, 1840);
    assert_eq!(modele, "mistral-small-3.2-24b");

    base.detruire().await;
}

#[tokio::test]
async fn tout_le_vocabulaire_rust_est_accepte_par_la_base() {
    // Le défaut que ce test attrape : une variante ajoutée à l'énumération Rust sans être
    // ajoutée au `check` de la migration. Elle compilerait, passerait la revue, et échouerait
    // à l'écriture — sur un chemin d'inscription de coût, c'est-à-dire là où personne ne
    // regarde avant la fin du mois.
    let base = base_prete().await;

    let types = [
        TypeAppel::Message,
        TypeAppel::Image,
        TypeAppel::Audio,
        TypeAppel::Extraction,
        TypeAppel::Compaction,
    ];
    let origines = [Origine::Reponse, Origine::Proactif, Origine::TacheFond];
    let statuts = [Statut::Ok, Statut::Echec, Statut::RejeteModeration];

    let mut ecrites = 0;
    for type_appel in types {
        for origine in origines {
            for statut in statuts {
                let mut ligne = appel(Decimal::new(1, 6));
                ligne.type_appel = type_appel;
                ligne.origine = origine;
                ligne.statut = statut;
                consommation::inscrire(base.pool(), &ligne)
                    .await
                    .unwrap_or_else(|erreur| {
                        panic!("la base refuse {type_appel:?}/{origine:?}/{statut:?} : {erreur}")
                    });
                ecrites += 1;
            }
        }
    }

    println!(
        "{ecrites} combinaisons écrites : {} types × {} origines × {} statuts",
        types.len(),
        origines.len(),
        statuts.len()
    );
    assert_eq!(ecrites, 45);

    base.detruire().await;
}

#[tokio::test]
async fn une_ligne_de_cout_ne_se_supprime_ni_ne_se_reecrit() {
    let base = base_prete().await;
    let id = consommation::inscrire(base.pool(), &appel(Decimal::new(247, 6)))
        .await
        .expect("ligne inscrite");

    let suppression = sqlx::query("delete from consommation where id = $1")
        .bind(id)
        .execute(base.pool())
        .await;
    let reecriture = sqlx::query("update consommation set cout_fournisseur_eur = 0 where id = $1")
        .bind(id)
        .execute(base.pool())
        .await;

    println!("suppression : {}", message(&suppression));
    println!("réécriture  : {}", message(&reecriture));
    assert!(suppression.is_err(), "un registre ne se supprime pas");
    assert!(reecriture.is_err(), "un montant ne se réécrit pas");

    // Et la ligne est intacte : le refus n'est pas seulement une erreur, c'est une absence
    // d'effet.
    let reste: Decimal =
        sqlx::query_scalar("select cout_fournisseur_eur from consommation where id = $1")
            .bind(id)
            .fetch_one(base.pool())
            .await
            .expect("la ligne est toujours là");
    println!("montant après les deux tentatives : {reste} €");
    assert_eq!(reste, Decimal::new(247, 6));

    base.detruire().await;
}

#[tokio::test]
async fn la_purge_rgpd_detache_la_ligne_sans_perdre_le_montant() {
    // « Conserver uniquement ce que la comptabilité impose, sous forme anonymisée » : c'est la
    // seule mutation que la table admette, et elle doit rester possible — sans quoi la purge
    // n'aurait d'autre choix que de supprimer, donc de fausser la marge de la période.
    let base = base_prete().await;
    let id = consommation::inscrire(base.pool(), &appel(Decimal::new(247, 6)))
        .await
        .expect("ligne inscrite");

    // La forme exacte : détacher les trois rattachements, poser l'horodatage.
    sqlx::query(
        "update consommation
            set utilisateur_id = null, conversation_id = null, message_id = null,
                anonymisee_le = now()
          where id = $1",
    )
    .bind(id)
    .execute(base.pool())
    .await
    .expect("l'anonymisation est la mutation admise");

    let (utilisateur, montant, modele): (Option<i64>, Decimal, String) = sqlx::query_as(
        "select utilisateur_id, cout_fournisseur_eur, modele from consommation where id = $1",
    )
    .bind(id)
    .fetch_one(base.pool())
    .await
    .expect("ligne relue");

    println!("après purge — utilisateur : {utilisateur:?} | montant : {montant} € | modèle : {modele}");
    assert_eq!(utilisateur, None, "la ligne ne désigne plus personne");
    assert_eq!(montant, Decimal::new(247, 6), "le montant survit à la purge");

    // Et l'utilisateur peut alors être supprimé : la clé étrangère ne le retient plus. C'est
    // la raison pour laquelle `utilisateur_id` a dû devenir nullable.
    sqlx::query("delete from utilisateurs where id = $1")
        .bind(UTILISATEUR)
        .execute(base.pool())
        .await
        .expect("plus rien ne retient l'utilisateur");
    println!("utilisateur supprimé, la ligne de coût demeure");

    // Une ligne déjà anonymisée ne bouge plus du tout.
    let seconde = sqlx::query("update consommation set anonymisee_le = now() where id = $1")
        .bind(id)
        .execute(base.pool())
        .await;
    println!("seconde anonymisation : {}", message(&seconde));
    assert!(seconde.is_err());

    base.detruire().await;
}

#[tokio::test]
async fn le_cout_d_une_periode_se_somme_et_vaut_zero_quand_rien_n_a_ete_consomme() {
    let base = base_prete().await;
    let debut = Utc::now() - DureeChrono::hours(1);

    let vide = consommation::cout_depuis(base.pool(), UTILISATEUR, debut)
        .await
        .expect("somme lue");
    println!("avant tout appel : {vide} €");
    assert_eq!(vide, Decimal::ZERO, "« rien consommé » se lit zéro, pas absent");

    for montant in [Decimal::new(247, 6), Decimal::new(1_531, 6), Decimal::new(89, 6)] {
        consommation::inscrire(base.pool(), &appel(montant))
            .await
            .expect("ligne inscrite");
    }

    let total = consommation::cout_depuis(base.pool(), UTILISATEUR, debut)
        .await
        .expect("somme lue");
    let futur = consommation::cout_depuis(base.pool(), UTILISATEUR, Utc::now() + DureeChrono::hours(1))
        .await
        .expect("somme lue");

    println!("0,000247 + 0,001531 + 0,000089 = {total} €");
    println!("sur une période à venir : {futur} €");
    assert_eq!(total, Decimal::new(1_867, 6));
    assert_eq!(futur, Decimal::ZERO, "la borne de date doit être appliquée");

    // Trois messages coûtent 0,0019 € : mille messages coûtent 0,62 €. C'est l'ordre de
    // grandeur que la phase 1.6 devra confronter au prix d'un abonnement.
    println!("extrapolation : 1000 messages ≈ {} €", total / Decimal::from(3) * Decimal::from(1000));

    base.detruire().await;
}

/// Le message d'une erreur `sqlx`, ou « accepté » si l'écriture est passée.
fn message<T>(resultat: &Result<T, sqlx::Error>) -> String {
    match resultat {
        Ok(_) => "ACCEPTÉ (ce qui est le défaut)".to_owned(),
        Err(erreur) => format!("refusé — {erreur}"),
    }
}
