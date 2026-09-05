//! Les vocabulaires contrôlés dans lesquels l'utilisateur choisit.
//!
//! # Pourquoi lire un catalogue plutôt qu'accepter du texte
//!
//! Rien de ce qui alimente le prompt système n'est saisi : l'utilisateur désigne des lignes de
//! ces tables, et le service compose à partir de leurs descriptions — écrites une fois, validées
//! une fois. La conséquence qui compte : **si aucune valeur du catalogue n'évoque un mineur,
//! aucune composition ne le peut.** L'interdiction absolue du projet cesse d'être un filtre
//! qu'on espère fiable pour devenir une propriété de l'ensemble des valeurs possibles.
//!
//! C'est aussi ce qui rend le retrait d'une option instantané et rétroactif : passer `actif` à
//! faux, et plus aucune composition ne la reprend.

use rust_decimal::Decimal;
use sqlx::PgPool;

use super::ErreurBase;

/// Un choix simple : ce que l'utilisateur voit, et le code stable qui le désigne.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Choix {
    /// Identifiant stable, employé par le code et les tests.
    pub code: String,
    /// Ce qui s'affiche.
    pub libelle: String,
}

/// Un trait de caractère ou un ton : un choix, plus la description injectée dans le prompt.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Trait {
    /// Identifiant stable.
    pub code: String,
    /// Ce qui s'affiche.
    pub libelle: String,
    /// Le texte éditorial repris dans le prompt système.
    pub description: String,
}

/// Une tranche d'âge apparente. `age_min` ne descend jamais sous 25, la base l'interdit.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrancheAge {
    /// Identifiant stable.
    pub code: String,
    /// Ce qui s'affiche.
    pub libelle: String,
    /// Âge plancher de la tranche.
    pub age_min: i16,
}

/// Une combinaison nommée de deux traits.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Fusion {
    /// Le nom de la combinaison — « Yandere », « Flegmatique ».
    pub nom_fusion: String,
    /// La description qui remplace l'addition des deux descriptions simples.
    pub description_fusion: String,
}

/// Un curseur continu, sa valeur par défaut, et s'il se plafonne par pays.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ParametreGradue {
    /// Identifiant stable — `humour`, `intensite_suggestive`…
    pub code: String,
    /// Ce qui s'affiche.
    pub libelle: String,
    /// `personnalite`, `contenu` ou `proactivite`.
    pub domaine: String,
    /// Valeur retenue quand l'utilisateur n'en choisit pas.
    pub valeur_defaut: Decimal,
    /// Vrai si un pays peut en abaisser le maximum.
    pub plafonnable_juridiction: bool,
}

/// Les catalogues d'apparence, qui partagent tous la même forme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalogue {
    /// `ref_genres`.
    Genres,
    /// `ref_morphologies`.
    Morphologies,
    /// `ref_couleurs_cheveux`.
    CouleursCheveux,
    /// `ref_couleurs_yeux`.
    CouleursYeux,
    /// `ref_styles_vestimentaires`.
    StylesVestimentaires,
}

impl Catalogue {
    /// Le nom de table correspondant.
    ///
    /// Interpolé dans la requête, ce qui n'est sûr **que** parce que la valeur vient d'un
    /// énuméré fermé : aucune chaîne extérieure ne peut atteindre cet endroit. Le jour où ce
    /// serait un paramètre, ce serait une injection.
    const fn table(self) -> &'static str {
        match self {
            Self::Genres => "ref_genres",
            Self::Morphologies => "ref_morphologies",
            Self::CouleursCheveux => "ref_couleurs_cheveux",
            Self::CouleursYeux => "ref_couleurs_yeux",
            Self::StylesVestimentaires => "ref_styles_vestimentaires",
        }
    }

    /// Tous les catalogues d'apparence, pour les parcourir.
    #[must_use]
    pub const fn tous() -> [Self; 5] {
        [
            Self::Genres,
            Self::Morphologies,
            Self::CouleursCheveux,
            Self::CouleursYeux,
            Self::StylesVestimentaires,
        ]
    }
}

/// Les options actives d'un catalogue d'apparence, par ordre d'affichage.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn lister(pool: &PgPool, catalogue: Catalogue) -> Result<Vec<Choix>, ErreurBase> {
    Ok(sqlx::query_as(&format!(
        "select code, libelle from {} where actif order by libelle",
        catalogue.table()
    ))
    .fetch_all(pool)
    .await?)
}

/// Les tranches d'âge apparentes actives, de la plus jeune à la plus âgée.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn tranches_age(pool: &PgPool) -> Result<Vec<TrancheAge>, ErreurBase> {
    Ok(sqlx::query_as(
        "select code, libelle, age_min from ref_tranches_age_apparent
         where actif order by age_min",
    )
    .fetch_all(pool)
    .await?)
}

/// Les archétypes actifs.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn archetypes(pool: &PgPool) -> Result<Vec<Trait>, ErreurBase> {
    traits_de(pool, "ref_archetypes").await
}

/// Les tons actifs.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn tons(pool: &PgPool) -> Result<Vec<Trait>, ErreurBase> {
    traits_de(pool, "ref_tons").await
}

/// La fusion d'archétypes correspondant à ce couple **orienté**, s'il en existe une.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn fusion_archetypes(
    pool: &PgPool,
    principal: &str,
    secondaire: &str,
) -> Result<Option<Fusion>, ErreurBase> {
    fusion_de(pool, "ref_fusions_archetypes", principal, secondaire).await
}

/// La fusion de tons correspondant à ce couple **orienté**, s'il en existe une.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn fusion_tons(
    pool: &PgPool,
    principal: &str,
    secondaire: &str,
) -> Result<Option<Fusion>, ErreurBase> {
    fusion_de(pool, "ref_fusions_tons", principal, secondaire).await
}

/// Tous les curseurs actifs, avec leur défaut.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn parametres_gradues(pool: &PgPool) -> Result<Vec<ParametreGradue>, ErreurBase> {
    Ok(sqlx::query_as(
        "select code, libelle, domaine, valeur_defaut, plafonnable_juridiction
         from ref_parametres_gradues where actif order by domaine, code",
    )
    .fetch_all(pool)
    .await?)
}

/// Le plafond que ce pays impose à ce curseur, s'il en impose un.
///
/// Aucune ligne signifie « aucun plafond », ce qui est le bon défaut pour un pays où le service
/// n'est pas ouvert — à condition, précisément, de ne pas l'y ouvrir. Le peuplement de cette
/// table est un processus opérationnel avec revue légale, pas un champ technique.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn plafond(
    pool: &PgPool,
    code_pays: &str,
    parametre_code: &str,
) -> Result<Option<Decimal>, ErreurBase> {
    Ok(sqlx::query_scalar(
        "select valeur_max from ref_plafonds_juridiction
         where code_pays = $1 and parametre_code = $2",
    )
    .bind(code_pays)
    .bind(parametre_code)
    .fetch_optional(pool)
    .await?)
}

/// Le corps commun des deux catalogues de traits.
async fn traits_de(pool: &PgPool, table: &'static str) -> Result<Vec<Trait>, ErreurBase> {
    Ok(sqlx::query_as(&format!(
        "select code, libelle, description from {table} where actif order by libelle"
    ))
    .fetch_all(pool)
    .await?)
}

/// Le corps commun des deux tables de fusion.
async fn fusion_de(
    pool: &PgPool,
    table: &'static str,
    principal: &str,
    secondaire: &str,
) -> Result<Option<Fusion>, ErreurBase> {
    Ok(sqlx::query_as(&format!(
        "select nom_fusion, description_fusion from {table}
         where code_principal = $1 and code_secondaire = $2 and actif"
    ))
    .bind(principal)
    .bind(secondaire)
    .fetch_optional(pool)
    .await?)
}
