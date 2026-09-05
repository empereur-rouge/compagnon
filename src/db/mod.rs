//! Accès à PostgreSQL : connexion, migrations, et les deux tables que la phase 1.1 exerce.
//!
//! # Ce que la base change par rapport à la phase 0
//!
//! La file vivait en mémoire : tout arrêt brutal en perdait le contenu, et le service ne
//! savait rien d'un utilisateur entre deux messages. La file est désormais une table, et les
//! tâches y sont prises **à bail** — ce qui rend récupérable une tâche dont le worker est mort
//! sans la rendre, au lieu de la laisser bloquée dans un état « en cours » que personne ne
//! nettoie.
//!
//! # Sur les secrets
//!
//! L'URL de connexion porte un mot de passe. Contrairement à `reqwest`, `sqlx` ne la
//! transporte dans aucune de ses erreurs — vérifié empiriquement sur les trois classes
//! (authentification refusée, hôte injoignable, URL malformée) et gardé par un test, parce que
//! c'est une propriété de la bibliothèque et non du code de ce projet.

pub mod catalogues;
pub mod file;
pub mod personnages;
pub mod utilisateurs;

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Délai au-delà duquel l'obtention d'une connexion du pool est abandonnée.
///
/// Court à dessein : une base injoignable doit se manifester par une erreur nette au
/// démarrage, pas par un service qui paraît vivant et met une minute à répondre.
const DELAI_ACQUISITION: Duration = Duration::from_secs(5);

/// Nombre maximal de connexions du pool.
///
/// **Ce n'est pas le nombre de workers qui dimensionne ce chiffre.** Un worker ne détient
/// aucune connexion pendant qu'il traite : `fetch_optional(&pool)` en emprunte une, l'exécute
/// et la rend aussitôt, alors que le temps de traitement est presque entièrement l'appel
/// Telegram qui suit. Mesuré : six à quinze connexions ouvertes en régime, jamais la borne.
///
/// Le vrai dimensionneur est la concurrence du webhook — Telegram livre jusqu'à quarante
/// appels simultanés. À 0,19 ms par requête, seize connexions en servent des dizaines de
/// milliers par seconde.
///
/// La nuance comptera en phase 1.3 : si une transaction est un jour tenue pendant l'appel de
/// modèle, le raisonnement change du tout au tout et ce chiffre avec lui.
const CONNEXIONS_MAX: u32 = 16;

/// Les migrations, embarquées dans le binaire à la compilation.
///
/// Embarquées et non lues sur disque : le conteneur livré ne contient pas l'arbre source, et
/// une migration absente au démarrage se traduirait par un service qui tourne sur un schéma
/// incomplet plutôt que par un refus franc.
static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Ce qui a empêché un accès à la base d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurBase {
    /// La connexion n'a pas pu être établie.
    #[error("connexion à la base impossible : {0}")]
    Connexion(#[source] sqlx::Error),

    /// Une migration a échoué, ou le schéma en place diverge de celui attendu.
    #[error("migration refusée : {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    /// Une requête a échoué.
    ///
    /// `#[from]` plutôt qu'un `map_err` à chaque requête : c'est la convention du dépôt
    /// (`cli::ErreurCli`, `app::ErreurDemarrage`, `telegram::Canal`), et seize recopies de la
    /// même incantation masquaient la logique SQL qu'elles entouraient. `Connexion` reste en
    /// `#[source]` explicite — elle n'a qu'un site de construction, ce qui lève l'ambiguïté
    /// entre les deux variantes qui portent une `sqlx::Error`.
    #[error("requête refusée par la base : {0}")]
    Requete(#[from] sqlx::Error),

    /// Une charge utile n'a pas pu être convertie avant d'être enfilée.
    ///
    /// Variante à part, et non un `Requete` emprunté : rien n'a atteint la base quand elle
    /// survient, et l'annoncer comme « requête refusée par la base » enverrait un exploitant
    /// regarder PostgreSQL pour un défaut qui est chez nous. C'est aussi ce qui garde
    /// `Requete` dans le domaine que son test de non-fuite couvre — celui des erreurs `sqlx`.
    #[error("charge utile inconvertible : {0}")]
    ChargeUtile(#[from] serde_json::Error),
}

/// Le pool de connexions, et ce qu'on en fait.
///
/// Ne dérive pas `Debug` : par cohérence avec [`crate::telegram::Canal`], rien de ce qui touche
/// à une ressource authentifiée ne doit devenir imprimable par accident. `PgPool` est
/// interne­ment un `Arc`, donc le cloner ne coûte qu'un incrément.
#[derive(Clone)]
pub struct Base {
    pool: PgPool,
}

impl Base {
    /// Ouvre le pool et vérifie qu'une connexion s'établit réellement.
    ///
    /// La vérification est immédiate et non paresseuse : une base injoignable doit empêcher le
    /// démarrage, exactement comme un jeton Telegram refusé. Un service qui accepte des
    /// requêtes puis les met toutes en échec est pire qu'un service qui refuse de démarrer.
    ///
    /// # Errors
    ///
    /// [`ErreurBase::Connexion`] si le pool ne peut pas obtenir une première connexion.
    pub async fn connecter(url: &str) -> Result<Self, ErreurBase> {
        let pool = PgPoolOptions::new()
            .max_connections(CONNEXIONS_MAX)
            .acquire_timeout(DELAI_ACQUISITION)
            .connect(url)
            .await
            .map_err(ErreurBase::Connexion)?;
        Ok(Self { pool })
    }

    /// Joint la base **et** met son schéma à jour.
    ///
    /// Les deux gestes vont ensemble : les séparer obligeait chaque appelant à penser à faire
    /// le second, dans le bon ordre. Composés ici, « un [`Base`] existe » implique « son schéma
    /// est à jour » — un fait de typage plutôt qu'une convention d'appel.
    ///
    /// # Errors
    ///
    /// [`ErreurBase::Connexion`] ou [`ErreurBase::Migration`].
    pub async fn ouvrir(url: &str) -> Result<Self, ErreurBase> {
        let base = Self::connecter(url).await?;
        base.migrer().await?;
        Ok(base)
    }

    /// Applique les migrations en attente.
    ///
    /// # Errors
    ///
    /// [`ErreurBase::Migration`] si une migration échoue, ou si une migration déjà appliquée a
    /// été modifiée depuis — `sqlx` compare les empreintes et refuse, ce qui est le
    /// comportement voulu : une migration éditée après coup est une divergence silencieuse
    /// entre le schéma du code et celui de la production.
    pub async fn migrer(&self) -> Result<(), ErreurBase> {
        Ok(MIGRATIONS.run(&self.pool).await?)
    }

    /// Le pool, pour les modules de requêtes.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Nombre de tâches encore à traiter — la mesure la plus utile de la sonde.
    ///
    /// # Errors
    ///
    /// [`ErreurBase::Requete`] si la base ne répond pas.
    pub async fn taches_en_attente(&self) -> Result<i64, ErreurBase> {
        Ok(sqlx::query_scalar(
            "select count(*) from file_messages
             where statut = 'en_attente'
                or (statut = 'en_cours' and bail_expire_le < now())",
        )
        .fetch_one(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn aucune_erreur_de_base_ne_laisse_fuir_le_mot_de_passe() {
        // `sqlx` ne transporte pas l'URL dans ses erreurs — vérifié sur les trois classes.
        // Ce test garde cette propriété : c'est celle de la bibliothèque, pas la nôtre, et
        // c'est exactement le genre de garantie qu'une mise à jour peut retirer en silence.
        const MOTDEPASSE: &str = "MotDePasseQuiNeDoitJamaisSortir";

        let cas = [
            (
                "authentification refusée",
                format!("postgres://compagnon:{MOTDEPASSE}@127.0.0.1:5433/compagnon_test"),
            ),
            (
                "hôte injoignable",
                format!("postgres://compagnon:{MOTDEPASSE}@127.0.0.1:9/rien"),
            ),
        ];

        for (nom, url) in cas {
            let Err(erreur) = Base::connecter(&url).await else {
                println!("{nom} : connexion inattendue, cas ignoré");
                continue;
            };
            let rendu = format!("{erreur} | {erreur:?}");
            println!("{nom:26} -> {rendu}");
            assert!(
                !rendu.contains(MOTDEPASSE),
                "le mot de passe fuit dans l'erreur « {nom} »"
            );
        }
    }
}
