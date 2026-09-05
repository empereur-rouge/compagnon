//! Ce qu'on vérifie avant qu'un compagnon puisse parler.
//!
//! # Ce qui est structurel, et ce qui ne l'est pas
//!
//! La distinction est le sujet de ce module, et il faut la tenir honnêtement.
//!
//! **Structurel** : tout ce qui alimente le prompt vient de catalogues clos. Une apparence ne
//! peut pas évoquer un mineur parce que `ref_tranches_age_apparent` refuse toute ligne sous
//! 25 ans ; une personnalité ne peut pas dériver parce que les descriptions sont écrites au
//! catalogue et validées une fois. Ce ne sont pas des filtres, ce sont des ensembles de valeurs
//! possibles. Rien à examiner.
//!
//! **Heuristique** : le **nom**. C'est le seul texte libre d'un compagnon, donc le seul
//! interstice, et ce qui suit est ce qu'on y fait. Une partie en est structurelle — un nom ne
//! peut contenir aucun chiffre, ce qui élimine d'un coup toute la classe « lea12ans » sans
//! avoir à en énumérer les graphies. Le reste est un rapprochement de termes, avec les limites
//! d'un rapprochement de termes : il rate les graphies détournées, les diminutifs, les langues
//! absentes de la liste.
//!
//! **Ce module n'est donc pas le classifieur du produit.** Il est la première ligne, et il est
//! délibérément conservateur — un faux refus coûte un nom à changer, un faux accord coûte
//! infiniment plus. Le contrôle réel arrive avec le client de modèle en phase 1.3, où le nom
//! pourra être soumis avec son contexte.

use sqlx::PgPool;

use crate::db::ErreurBase;

/// Longueur minimale d'un nom, en caractères.
const NOM_MIN: usize = 2;

/// Longueur maximale d'un nom, en caractères.
///
/// Assez pour « Marie-Ange de la Tour », trop peu pour y glisser une consigne au modèle — le
/// nom est repris dans le prompt, et un nom de trois cents caractères en serait un fragment.
const NOM_MAX: usize = 32;

/// En deçà de cette longueur, un terme n'est cherché que comme mot entier.
///
/// « mere » comme sous-chaîne attraperait « Meredith », « ado » attraperait « Adolphe ». Un
/// terme court ne se rapproche donc que d'un mot complet ; au-delà de ce seuil, la
/// concaténation devient le contournement évident (« petitefille ») et la sous-chaîne le bon
/// outil.
const LONGUEUR_SOUS_CHAINE: usize = 6;

/// Ce qu'a décidé la modération.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Le compagnon peut être activé.
    Accepte,
    /// Refusé, avec de quoi le dire à l'utilisateur sans le renseigner.
    Refuse(Motif),
}

/// Pourquoi un nom a été refusé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Motif {
    /// Trop court pour être un nom.
    TropCourt,
    /// Trop long pour tenir dans un prompt sans en devenir un fragment.
    TropLong,
    /// Contient un chiffre. Refusé sans exception : c'est ce qui couvre les mentions d'âge.
    ContientUnChiffre,
    /// Contient un caractère qui n'a rien à faire dans un nom.
    CaractereInterdit,
    /// Rapproché d'un terme de la liste.
    ///
    /// Le terme n'est **pas** rendu à l'utilisateur : le lui dire lui apprendrait exactement
    /// quoi contourner. Il est journalisé.
    TermeInterdit {
        /// Le terme reconnu, pour le journal d'exploitation.
        terme: String,
        /// Sa catégorie.
        motif: String,
    },
}

impl Motif {
    /// Ce qu'on dit à l'utilisateur — assez pour corriger, pas assez pour contourner.
    #[must_use]
    pub const fn message_public(&self) -> &'static str {
        match self {
            Self::TropCourt => "Ce nom est trop court.",
            Self::TropLong => "Ce nom est trop long.",
            Self::ContientUnChiffre => "Un nom ne peut pas contenir de chiffre.",
            Self::CaractereInterdit => "Ce nom contient un caractère qui n'est pas accepté.",
            Self::TermeInterdit { .. } => "Ce nom ne peut pas être retenu. Choisis-en un autre.",
        }
    }
}

/// Réduit un nom à sa forme comparable : minuscules, sans accents, sans séparateurs.
///
/// « Petite-Fille », « petite fille » et « PetiteFille » donnent la même chaîne, donc le même
/// verdict. Sans cette normalisation, la liste serait contournable par un tiret.
#[must_use]
pub fn normaliser(nom: &str) -> String {
    nom.chars()
        .filter_map(|c| {
            let c = c.to_lowercase().next().unwrap_or(c);
            match c {
                'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => Some('a'),
                'ç' => Some('c'),
                'é' | 'è' | 'ê' | 'ë' => Some('e'),
                'î' | 'ï' | 'í' | 'ì' => Some('i'),
                'ô' | 'ö' | 'ó' | 'ò' | 'õ' => Some('o'),
                'ù' | 'û' | 'ü' | 'ú' => Some('u'),
                'ÿ' | 'ý' => Some('y'),
                'ñ' => Some('n'),
                'æ' => Some('a'),
                'œ' => Some('o'),
                c if c.is_alphanumeric() => Some(c),
                // Tout séparateur disparaît : c'est ce qui rend « petite-fille » comparable à
                // « petitefille ».
                _ => None,
            }
        })
        .collect()
}

/// Découpe un nom en mots, pour le rapprochement des termes courts.
fn mots(nom: &str) -> Vec<String> {
    nom.split(|c: char| !c.is_alphanumeric())
        .filter(|m| !m.is_empty())
        .map(normaliser)
        .collect()
}

/// Examine un nom de compagnon.
///
/// Les contrôles de forme sont faits avant la lecture de la liste : ils ne coûtent rien et
/// évitent un aller-retour en base pour un nom manifestement irrecevable.
///
/// # Errors
///
/// [`ErreurBase`] si la liste de termes n'est pas lisible. Une liste inaccessible n'est **pas**
/// traitée comme une liste vide : sans elle, on ne peut rien affirmer, et le seul comportement
/// acceptable est de refuser de conclure.
pub async fn examiner_nom(pool: &PgPool, nom: &str) -> Result<Verdict, ErreurBase> {
    let ajuste = nom.trim();

    let longueur = ajuste.chars().count();
    if longueur < NOM_MIN {
        return Ok(Verdict::Refuse(Motif::TropCourt));
    }
    if longueur > NOM_MAX {
        return Ok(Verdict::Refuse(Motif::TropLong));
    }
    if ajuste.chars().any(char::is_numeric) {
        return Ok(Verdict::Refuse(Motif::ContientUnChiffre));
    }
    // Ce qui n'est ni lettre, ni espace, ni tiret, ni apostrophe n'a rien à faire dans un nom —
    // et ferme au passage les caractères de contrôle et les tentatives d'y glisser du balisage.
    if ajuste
        .chars()
        .any(|c| !c.is_alphabetic() && !" -'’".contains(c))
    {
        return Ok(Verdict::Refuse(Motif::CaractereInterdit));
    }

    let normalise = normaliser(ajuste);
    let mots = mots(ajuste);

    let termes: Vec<(String, String)> =
        sqlx::query_as("select terme, motif from ref_termes_interdits where actif")
            .fetch_all(pool)
            .await?;

    for (terme, motif) in termes {
        let reconnu = if terme.chars().count() >= LONGUEUR_SOUS_CHAINE {
            normalise.contains(&terme)
        } else {
            mots.iter().any(|mot| mot == &terme)
        };
        if reconnu {
            return Ok(Verdict::Refuse(Motif::TermeInterdit { terme, motif }));
        }
    }

    Ok(Verdict::Accepte)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn la_normalisation_rend_les_contournements_par_separateur_inoperants() {
        for forme in [
            "Petite-Fille",
            "petite fille",
            "PetiteFille",
            "Pétite—Fille",
        ] {
            println!("{forme:16} -> {}", normaliser(forme));
            assert_eq!(normaliser(forme), "petitefille");
        }
    }

    #[test]
    fn les_termes_courts_ne_sont_pas_cherches_en_sous_chaine() {
        // C'est le piège de ce module : « mere » en sous-chaîne refuserait « Meredith »,
        // « ado » refuserait « Adolphe ». Un faux refus n'est pas gratuit — il fait échouer
        // quelqu'un qui n'a rien fait, sur son premier geste dans le produit.
        for nom in ["Meredith", "Adolphe", "Amadou", "Filsuvit", "Teodora"] {
            let mots = mots(nom);
            println!("{nom:10} -> mots {mots:?}");
            for court in ["mere", "ado", "fils", "teen"] {
                assert!(
                    !mots.iter().any(|m| m == court),
                    "« {nom} » serait refusé à cause de « {court} »"
                );
            }
        }
    }

    #[test]
    fn les_termes_longs_sont_cherches_en_sous_chaine() {
        // À l'inverse, un terme long collé à un autre mot est le contournement évident.
        assert!(normaliser("MaPetiteFilleAdoree").contains("petitefille"));
        println!(
            "MaPetiteFilleAdoree -> {}",
            normaliser("MaPetiteFilleAdoree")
        );
    }
}
