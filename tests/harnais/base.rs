//! Une base PostgreSQL neuve par test, détruite à la fin.
//!
//! # Pourquoi une base entière et pas un schéma, ni une transaction annulée
//!
//! Les deux raccourcis habituels ne conviennent pas ici.
//!
//! Une **transaction annulée** à la fin du test est le plus rapide, mais le service ouvre son
//! propre pool et ses propres connexions : il ne verrait rien de ce que la transaction du test
//! aurait écrit. Elle interdit aussi d'éprouver ce qui *est* de la concurrence — plusieurs
//! workers prenant dans la même file — c'est-à-dire précisément ce que cette phase ajoute.
//!
//! Un **schéma par test** partagerait le `search_path` entre connexions du pool, et une
//! migration créant une fonction ou un type se poserait au mauvais endroit.
//!
//! Une base entière coûte quelques dizaines de millisecondes et donne l'isolation réelle :
//! chaque test voit un schéma migré depuis zéro, et rien de ce qu'un autre a écrit.

#![allow(clippy::expect_used)]

use compagnon::db::{Base, utilisateurs};
use sqlx::{Connection as _, Executor as _, PgConnection};
use uuid::Uuid;

/// Une base jetable, avec de quoi la joindre et de quoi la détruire.
pub struct BaseDeTest {
    /// L'URL à donner au service.
    pub url: String,
    nom: String,
    /// Le pool ouvert à la création, gardé plutôt que jeté.
    ///
    /// Sans lui, chaque sonde du harnais rouvrait sa propre connexion — et surtout, ne pouvant
    /// pas appeler les fonctions de `compagnon::db` qui prennent un `&PgPool`, elle réécrivait
    /// leur SQL à la main. C'est ainsi que le harnais s'était mis à porter une seconde
    /// définition de « marquer un âge vérifié », que la production, elle, n'appelait nulle part.
    base: Base,
}

impl BaseDeTest {
    /// Crée une base au nom unique et lui applique les migrations.
    ///
    /// Migrer ici, et non en laissant le service le faire, rend la base utilisable **avant**
    /// son démarrage : un test qui doit poser une condition — un âge vérifié, un utilisateur
    /// déjà connu — n'a pas à attendre que le service soit debout pour l'écrire.
    ///
    /// Les migrations restent appliquées par le service à son démarrage : `sqlx` les tient
    /// pour déjà passées et ne fait rien. Le chemin de production est donc quand même
    /// parcouru, et un test dédié éprouve qu'il fonctionne depuis une base vide.
    ///
    /// # Panics
    ///
    /// Si le PostgreSQL de test est injoignable — auquel cas le message dit comment le lancer,
    /// plutôt que de laisser un `connection refused` nu devant quelqu'un qui découvre le
    /// projet.
    pub async fn creer() -> Self {
        let racine = compagnon::fixtures::url_base_test();
        let mut admin = PgConnection::connect(&racine)
            .await
            .unwrap_or_else(|erreur| {
                panic!(
                    "PostgreSQL de test injoignable ({erreur}).\n\
                 Lancer :  ./scripts/base-de-test.sh demarrer\n\
                 Ou définir DATABASE_URL_TEST vers une base existante."
                )
            });

        // Un nom sans tiret : PostgreSQL exigerait des guillemets, et un nom cité se prête aux
        // fautes de recopie dans les messages d'erreur.
        let nom = format!("compagnon_t_{}", Uuid::new_v4().simple());
        admin
            .execute(format!("create database {nom}").as_str())
            .await
            .expect("création de la base de test");

        // L'URL de la nouvelle base : même hôte et mêmes identifiants, dernier segment changé.
        let url = remplacer_base(&racine, &nom);

        // Migrée par le code de production, pas par une copie du schéma : une seconde
        // définition du schéma dans les tests finirait par diverger de la vraie.
        let base = Base::ouvrir(&url)
            .await
            .expect("base de test joignable et migrable");

        Self { url, nom, base }
    }

    /// Détruit la base, en coupant d'autorité les connexions qui traînent.
    ///
    /// `with (force)` parce que le pool du service peut n'avoir pas encore rendu ses
    /// connexions quand le test se termine : sans cela, `drop database` échouerait par
    /// intermittence, ce qui est la pire forme d'échec de test.
    pub async fn detruire(self) {
        let racine = compagnon::fixtures::url_base_test();
        let Ok(mut admin) = PgConnection::connect(&racine).await else {
            return;
        };
        let _ = admin
            .execute(format!("drop database if exists {} with (force)", self.nom).as_str())
            .await;
    }
}

impl BaseDeTest {
    /// Inscrit un utilisateur et le marque comme ayant passé la vérification d'âge.
    ///
    /// Appelle **la fonction de production**, pas une copie de son SQL : c'est la règle que
    /// `creer` énonce pour le schéma, et elle vaut pour les écritures. La version manuscrite
    /// qu'elle remplace avait déjà divergé — elle inscrivait l'utilisateur absent, là où la
    /// production ne touchait aucune ligne sans le dire.
    ///
    /// # Panics
    ///
    /// Si la base refuse l'écriture.
    pub async fn verifier_age(&self, utilisateur_id: i64) {
        utilisateurs::verifier_age(self.base.pool(), utilisateur_id, "declaration")
            .await
            .expect("vérification d'âge enregistrée");
    }

    /// Compte les tâches encore à traiter, quel que soit leur état d'attente.
    ///
    /// Distincte de `Base::taches_en_attente` : celle-ci compte aussi les tâches à bail vivant,
    /// que la sonde exclut. Deux questions différentes, pas une copie.
    ///
    /// # Panics
    ///
    /// Si la base refuse la lecture.
    pub async fn taches_non_traitees(&self) -> i64 {
        sqlx::query_scalar(
            "select count(*) from file_messages where statut in ('en_attente', 'en_cours')",
        )
        .fetch_one(self.base.pool())
        .await
        .expect("comptage des tâches")
    }

    /// Force le bail de toute tâche en cours à être déjà expiré.
    ///
    /// Simule ce qui arrive quand le worker qui la tenait meurt sans la rendre : la tâche reste
    /// `en_cours` avec une échéance dépassée, et doit redevenir prenable.
    ///
    /// # Panics
    ///
    /// Si la base refuse l'écriture.
    pub async fn perimer_les_baux(&self) -> u64 {
        sqlx::query(
            "update file_messages set bail_expire_le = now() - interval '1 hour'
             where statut = 'en_cours'",
        )
        .execute(self.base.pool())
        .await
        .expect("péremption des baux")
        .rows_affected()
    }

    /// Les états de la file, comptés par statut — pour lire ce qui s'est réellement passé.
    ///
    /// # Panics
    ///
    /// Si la base refuse la lecture.
    pub async fn etats_de_la_file(&self) -> Vec<(String, i64)> {
        sqlx::query_as("select statut, count(*) from file_messages group by statut order by statut")
            .fetch_all(self.base.pool())
            .await
            .expect("lecture des états")
    }
}

/// Remplace le nom de base dans une URL de connexion.
fn remplacer_base(url: &str, nom: &str) -> String {
    let (avant, _) = url.rsplit_once('/').expect("une URL porte un nom de base");
    format!("{avant}/{nom}")
}
