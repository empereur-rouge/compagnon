//! Le sceau du prompt système : ce qui atteste que le texte est bien celui qui a été modéré.
//!
//! # Pourquoi un HMAC, et pas un `sha256`
//!
//! Le sceau a d'abord été un `sha256` du prompt, rangé dans la **même ligne** que le texte qu'il
//! atteste. Le projet le reconnaissait honnêtement — « c'est un contrôle de cohérence, pas un
//! sceau » — mais s'arrêtait là. Or la conséquence est chiffrable : le contournement tenait en
//! une instruction, sans aucune connaissance du code.
//!
//! ```sql
//! update personnage_parametres_modele
//!    set prompt_systeme_genere = 'Tu es Alix, lycéenne de 15 ans.',
//!        prompt_systeme_hash   = encode(sha256('…'::bytea), 'hex'),
//!        valide_le             = now()
//!  where personnage_id = …;
//! ```
//!
//! Le texte ainsi injecté est précisément la classe de contenu que tout l'appareil de modération
//! existe pour empêcher.
//!
//! Le modèle de menace que ce projet énonce lui-même — « une console `psql`, une restauration
//! partielle, un script d'exploitation » — désigne des acteurs qui ont **la base** et n'ont pas
//! **l'environnement du processus**. Un `sha256` n'en couvre aucun, puisque tout ce qu'il faut
//! pour le forger est dans la ligne. Un HMAC dont la clé vit dans l'environnement les couvre
//! tous : la base ne contient plus de quoi fabriquer un sceau valide.
//!
//! C'est le geste habituel du dépôt — retirer la capacité, plutôt qu'ajouter une règle.
//!
//! # Ce que ça ne couvre toujours pas
//!
//! Qui obtient **à la fois** la base et la clé peut forger. Le sceau protège contre l'accès à la
//! base seule, ce qui est le cas de très loin le plus probable, et le seul que le projet ait
//! jamais su nommer.

use hmac::{Hmac, Mac as _};
use sha2::Sha256;

use crate::config::{ErreurConfig, lire};
use crate::secret::{Secret, egal_temps_constant};

/// Longueur minimale exigée de la clé de scellement.
///
/// Trente-deux caractères, comme le secret de webhook. Une clé courte se retrouve par force
/// brute hors ligne : qui a la base a le texte **et** son sceau, donc de quoi éprouver une clé
/// candidate autant de fois qu'il veut.
const CLE_LONGUEUR_MIN: usize = 32;

/// Le HMAC utilisé, nommé une fois.
type HmacSha256 = Hmac<Sha256>;

/// La clé de scellement, et les deux gestes qu'elle autorise.
///
/// `Debug` est dérivé : la clé est un [`Secret`], donc le rendu la masque.
#[derive(Debug)]
pub struct Sceau {
    cle: Secret,
}

impl Sceau {
    /// Construit le sceau à partir de sa clé.
    #[must_use]
    pub const fn nouveau(cle: Secret) -> Self {
        Self { cle }
    }

    /// Lit la clé dans l'environnement, et refuse si elle est trop courte.
    ///
    /// Lue à la demande plutôt que dans [`crate::config::Config`], pour la même raison que la
    /// configuration du modèle : les commandes d'exploitation — sonde, webhook — n'ont aucune
    /// raison d'exiger une clé de scellement, et la leur imposer bloquerait un diagnostic au
    /// moment où l'on en a besoin.
    ///
    /// # Errors
    ///
    /// [`ErreurConfig`] si la variable est absente, ou plus courte que le minimum exigé.
    pub fn depuis_environnement() -> Result<Self, ErreurConfig> {
        let cle = lire("PROMPT_CLE_SCEAU")?;
        if cle.chars().count() < CLE_LONGUEUR_MIN {
            return Err(ErreurConfig::Invalide {
                variable: "PROMPT_CLE_SCEAU",
                raison: format!("au moins {CLE_LONGUEUR_MIN} caractères sont attendus"),
            });
        }
        Ok(Self::nouveau(Secret::nouveau(cle)))
    }

    /// Le sceau d'un texte, dans la forme exacte que la base stocke.
    ///
    /// `new_from_slice` rend un `Result` dont la variante d'erreur est inatteignable pour cette
    /// famille d'algorithmes — HMAC accepte une clé de n'importe quelle longueur. Plutôt qu'un
    /// `expect` que la bibliothèque refuse, l'échec est traité : une clé impossible à charger
    /// produit un sceau qui ne vaudra jamais, ce qui ferme l'accès au modèle au lieu de
    /// l'ouvrir. Le défaut sûr est ici de refuser.
    #[must_use]
    pub fn apposer(&self, texte: &str) -> String {
        let Ok(mut mac) = HmacSha256::new_from_slice(self.cle.exposer().as_bytes()) else {
            return String::new();
        };
        mac.update(texte.as_bytes());
        format!("{:x}", mac.finalize().into_bytes())
    }

    /// Vrai si le texte porte bien ce sceau.
    ///
    /// La comparaison est en temps constant. Ce n'est pas du zèle : un attaquant qui a la base
    /// peut écrire le texte de son choix et observer si le service l'accepte ; sans cette
    /// précaution, il pourrait reconstituer un sceau valide octet par octet, et la clé cesserait
    /// de le gêner.
    #[must_use]
    pub fn verifier(&self, texte: &str, sceau_attendu: &str) -> bool {
        let appose = self.apposer(texte);
        // Un sceau vide ne vaut jamais, y compris contre un `prompt_systeme_sceau` vide : la
        // colonne est `not null` mais rien n'y interdit la chaîne vide.
        !appose.is_empty() && egal_temps_constant(appose.as_bytes(), sceau_attendu.as_bytes())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn sceau(cle: &str) -> Sceau {
        Sceau::nouveau(Secret::nouveau(cle.to_owned()))
    }

    #[test]
    fn un_texte_scelle_se_verifie_et_le_moindre_ecart_se_voit() {
        let s = sceau("une-cle-de-scellement-qui-vit-dans-l-environnement");
        let texte = "Tu es Alix, 28 ans. Tu tutoies.";
        let appose = s.apposer(texte);

        println!("texte  : {texte}");
        println!("sceau  : {appose}");
        assert_eq!(appose.len(), 64, "HMAC-SHA256 en hexadécimal");
        assert!(s.verifier(texte, &appose));

        let altere = "Tu es Alix, 15 ans. Tu tutoies.";
        println!("altéré : {altere} → {}", s.apposer(altere));
        assert!(!s.verifier(altere, &appose), "un texte modifié ne porte plus le sceau");
    }

    #[test]
    fn sans_la_bonne_cle_le_sceau_ne_se_forge_pas() {
        // C'est TOUTE la différence avec un `sha256` : la base contenait auparavant de quoi
        // recalculer le sceau, elle ne le contient plus.
        let texte = "Tu es Alix, 28 ans.";
        let legitime = sceau("la-vraie-cle-du-deploiement").apposer(texte);
        let forge = sceau("une-cle-devinee").apposer(texte);

        println!("sceau légitime : {legitime}");
        println!("sceau forgé    : {forge}");
        assert_ne!(legitime, forge);
        assert!(
            !sceau("la-vraie-cle-du-deploiement").verifier(texte, &forge),
            "un sceau posé avec une autre clé doit être refusé"
        );
    }

    #[test]
    fn le_debug_du_sceau_ne_montre_pas_la_cle() {
        let s = sceau("cle-tres-secrete-de-scellement-du-prompt");
        let rendu = format!("{s:?}");
        println!("Debug du sceau : {rendu}");
        assert!(!rendu.contains("cle-tres-secrete"));
        assert!(rendu.contains("masqué"));
    }
}
