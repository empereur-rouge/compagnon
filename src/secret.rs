//! Une valeur qui ne peut pas se retrouver dans un journal.
//!
//! # Pourquoi un type, et pas une convention
//!
//! Ce projet a déjà laissé fuir un secret **deux fois**, et les deux fois la règle était
//! écrite quelque part :
//!
//! - `ErreurEnvoi` conservait un `reqwest::Error`, dont le `Display` imprime l'URL — laquelle
//!   contient le jeton du bot. Une coupure réseau l'écrivait dans les journaux, que
//!   `compose.yaml` persiste sur disque.
//! - `masquer_url` découpait la chaîne sur `@` pour cacher le mot de passe de la base, et
//!   rendait l'URL **verbatim** quand elle n'en trouvait pas — sur deux formes que `sqlx`
//!   accepte, dont `postgres:///?password=…`.
//!
//! Dans les deux cas la garantie existait dans un commentaire et pas dans le code. Un type
//! règle cela une fois : ce qui n'a ni `Display`, ni `Debug` révélateur, ni `Serialize`, ni
//! `Deref` vers `str` ne peut pas atterrir dans un journal par distraction. Il faut appeler
//! [`Secret::exposer`], dont le nom est fait pour être cherché avec `rg` le jour d'un audit.
//!
//! # Ce que ce type ne protège pas
//!
//! Rien contre un `exposer()` suivi d'un `println!`. Il déplace la faute du silence vers un
//! appel explicite et nommé — c'est tout, et c'est déjà beaucoup : les deux fuites de ce projet
//! étaient silencieuses.

use std::fmt;

/// Une chaîne dont la sortie accidentelle est structurellement impossible.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Enveloppe une valeur.
    #[must_use]
    pub const fn nouveau(valeur: String) -> Self {
        Self(valeur)
    }

    /// Rend la valeur en clair.
    ///
    /// Le nom est délibérément désagréable : un `rg 'exposer\('` doit rendre la liste
    /// **exhaustive** des endroits où un secret sort de son enveloppe. Si cette liste dépasse
    /// une poignée de lignes, c'est le signe qu'il faut le passer plus loin enveloppé.
    #[must_use]
    pub fn exposer(&self) -> &str {
        &self.0
    }

    /// Vrai si la valeur est vide — pour valider une configuration sans la lire.
    #[must_use]
    pub fn est_vide(&self) -> bool {
        self.0.is_empty()
    }

    /// Le nombre de caractères, pour un message d'erreur qui aide sans divulguer.
    #[must_use]
    pub fn longueur(&self) -> usize {
        self.0.chars().count()
    }
}

/// `Debug` masque, et n'est pas dérivé : une dérivation imprimerait la valeur.
///
/// La longueur est rendue parce qu'elle aide au diagnostic — « clé de 0 caractère » et « clé de
/// 51 caractères » ne décrivent pas le même incident — et parce qu'elle ne divulgue rien.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<masqué, {} caractères>", self.longueur())
    }
}

// Pas d'implémentation de `Display` : c'est délibéré, et c'est la moitié de l'intérêt du type.
// `{}` sur un `Secret` ne compile pas, donc aucun `format!` de journal ne peut l'imprimer.
//
// Pas de `Deref<Target = str>` non plus, pour la même raison : il rendrait `&*secret` utilisable
// partout où une chaîne l'est, et ferait rentrer par la fenêtre ce que l'absence de `Display`
// fait sortir par la porte.

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn un_secret_ne_s_imprime_pas() {
        const VALEUR: &str = "sk-une-cle-qui-ne-doit-jamais-sortir";
        let secret = Secret::nouveau(VALEUR.to_owned());

        let rendu = format!("{secret:?}");
        println!("Debug d'un Secret : {rendu}");
        assert!(!rendu.contains(VALEUR), "le Debug divulgue la valeur");
        assert!(rendu.contains("36"), "la longueur aide au diagnostic et ne divulgue rien");

        // Et l'absence de `Display` est vérifiée par le compilateur : `format!("{secret}")`
        // ne compile pas. Aucun test ne peut l'éprouver — c'est justement ce qui en fait une
        // garantie plutôt qu'une vérification.
    }

    #[test]
    fn un_secret_imbrique_dans_une_structure_reste_masque() {
        // Le cas qui a réellement mordu : le secret n'est pas imprimé directement, il est
        // imprimé parce qu'il voyage dans une structure que quelqu'un journalise.
        // Les champs ne sont lus que par le `Debug` dérivé, ce que le détecteur de code mort
        // ne voit pas — et c'est précisément ce que le test éprouve.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Config {
            hote: String,
            cle: Secret,
        }

        let config = Config {
            hote: "https://api.exemple.fr".to_owned(),
            cle: Secret::nouveau("sk-secrete".to_owned()),
        };
        let rendu = format!("{config:?}");
        println!("Debug dérivé d'une structure qui en contient un :\n  {rendu}");
        assert!(!rendu.contains("sk-secrete"));
        assert!(rendu.contains("api.exemple.fr"), "le reste doit rester diagnosticable");
    }
}
