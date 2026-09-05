//! Le moteur qui écrit les réponses du compagnon.
//!
//! # Ce que ce module isole, et pourquoi
//!
//! Le fournisseur de calcul va changer — serverless au départ, GPU dédié quand le volume le
//! justifiera, et le modèle lui-même sera remplacé plusieurs fois. Le worker, lui, ne doit pas
//! bouger pour autant. D'où un trait, et une seule implémentation concrète derrière.
//!
//! Le trait sert aussi à autre chose, et c'est la raison qui compte le plus aujourd'hui : il
//! rend la **panne** du modèle éprouvable. Un modèle qui expire, qui refuse, qui rend du vide —
//! ce sont les cas où le worker doit se comporter correctement, et ils n'arrivent jamais quand
//! on les attend. Un double de test les produit à volonté.
//!
//! # Ce qui ne traverse pas ce module
//!
//! La clé d'accès au fournisseur ne sort jamais de son [`crate::secret::Secret`], et **aucune
//! variante d'erreur ne peut porter l'URL de l'appel**. Ce n'est pas de la prudence : dans ce
//! projet, un `reqwest::Error` conservé tel quel a déjà écrit un jeton dans les journaux. La
//! leçon a été tirée en classant la panne au lieu de la transporter — voir
//! [`crate::telegram::envoi::Panne`], dont [`Panne`] ici est le pendant.

#[cfg(any(test, feature = "fixtures"))]
pub mod double;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Qui parle, dans un tour de conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// L'humain.
    Utilisateur,
    /// Le compagnon.
    Compagnon,
}

/// Un tour de parole.
#[derive(Debug, Clone)]
pub struct Tour {
    /// Qui a parlé.
    pub role: Role,
    /// Ce qui a été dit.
    pub texte: String,
}

/// Tout ce que le modèle reçoit pour écrire une réponse.
#[derive(Debug, Clone)]
pub struct ContexteConversation {
    /// Le prompt système **validé**, lu tel quel en base.
    ///
    /// Lu, et non recomposé : c'est huit fois moins cher qu'un rechargement des traits, et
    /// surtout c'est le texte que la modération a réellement approuvé. Une recomposition
    /// pourrait en diverger — une description de catalogue modifiée, un plafond posé après
    /// coup — et le compagnon parlerait alors avec un prompt que personne n'a validé.
    pub prompt_systeme: String,

    /// Les tours précédents, du plus ancien au plus récent, le message courant en dernier.
    ///
    /// Vide de tout historique en phase 1.3 : la mémoire est la phase 2. Le champ existe
    /// maintenant pour que son arrivée ne change pas cette signature.
    pub echanges: Vec<Tour>,
}

/// Ce que le modèle a produit, et ce qu'il en a coûté.
#[derive(Debug, Clone)]
pub struct ReponseModele {
    /// Le texte à envoyer.
    pub texte: String,
    /// Identifiant exact du modèle qui a répondu — celui du fournisseur, pas celui demandé.
    ///
    /// Les deux peuvent différer : un alias côté hébergeur, une bascule de version. C'est
    /// celui-ci qu'on inscrit dans `consommation`, sinon la comparaison de coût entre deux
    /// versions repose sur ce qu'on croyait appeler.
    pub modele: String,
    /// Jetons consommés en entrée, si le fournisseur les rend.
    pub unites_entree: Option<i32>,
    /// Jetons produits, si le fournisseur les rend.
    pub unites_sortie: Option<i32>,
    /// Temps de l'appel, mesuré ici et non annoncé par le fournisseur.
    pub duree: Duration,
}

/// La nature d'un échec, sans rien de ce qui l'a causé.
///
/// Même forme et même raison que [`crate::telegram::envoi::Panne`] : classer plutôt que
/// transporter. Une erreur `reqwest` conservée telle quelle porte l'URL de l'appel, donc la
/// clé si elle y figure — et ce projet a déjà écrit un secret dans ses journaux par ce chemin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panne {
    /// L'appel n'a pas abouti dans le délai imparti.
    Delai,
    /// La connexion n'a pas pu être établie.
    Connexion,
    /// La réponse est arrivée mais n'a pas pu être lue.
    Corps,
    /// La requête n'a pas pu être formée ou émise.
    Requete,
    /// Rien de ce qui précède.
    Autre,
}

impl Panne {
    /// Libellé lisible en journal.
    #[must_use]
    pub const fn libelle(self) -> &'static str {
        match self {
            Self::Delai => "délai dépassé",
            Self::Connexion => "connexion impossible",
            Self::Corps => "réponse illisible",
            Self::Requete => "requête non émise",
            Self::Autre => "cause indéterminée",
        }
    }
}

impl std::fmt::Display for Panne {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.libelle())
    }
}

/// Ce qui a empêché le modèle de répondre.
#[derive(Debug, thiserror::Error)]
pub enum ErreurModele {
    /// Le fournisseur n'a pas répondu.
    #[error("modèle injoignable : {0}")]
    Injoignable(Panne),

    /// Le fournisseur a répondu, et a refusé.
    ///
    /// Le code HTTP seul, jamais le corps : un message d'erreur de fournisseur reprend souvent
    /// la requête, donc le prompt système, donc tout ce que le compagnon est.
    #[error("modèle indisponible : le fournisseur a répondu {code}")]
    Refuse {
        /// Le code de statut renvoyé.
        code: u16,
    },

    /// Le modèle a répondu, mais sans rien dire.
    ///
    /// Arrive plus souvent qu'on ne croit — un filtre côté fournisseur, une génération
    /// tronquée à zéro jeton. Distinguée d'un refus parce qu'elle se rejoue utilement.
    #[error("le modèle n'a rien produit")]
    Vide,
}

impl ErreurModele {
    /// Vrai si réessayer plus tard a une chance d'aboutir.
    ///
    /// Distinguer ces familles n'est pas cosmétique : réessayer un `401` refait la même erreur
    /// jusqu'à épuiser les tentatives, et abandonner un délai dépassé perd le message de
    /// quelqu'un qui l'attend.
    #[must_use]
    pub const fn merite_une_reprise(&self) -> bool {
        match self {
            // L'appel n'a pas abouti : l'état distant est inchangé.
            Self::Injoignable(_) | Self::Vide => true,
            Self::Refuse { code } => match *code {
                // Débit dépassé, ou défaillance côté fournisseur.
                429 | 500..=599 => true,
                // 400 (requête mal formée), 401/403 (clé invalide) : refaire le même appel
                // refera la même erreur.
                _ => false,
            },
        }
    }
}

/// Le moteur qui écrit, quel qu'il soit.
///
/// # Pourquoi une future encadrée plutôt qu'un `async fn`
///
/// Un `async fn` en trait ne rend pas le trait utilisable derrière un `dyn`. Or le worker doit
/// **porter** un client sans être générique : le rendre générique propagerait le paramètre de
/// type jusqu'à l'état partagé du service et jusqu'au routeur.
///
/// L'alternative habituelle est la caisse `async-trait`. Elle produit exactement le code
/// ci-dessous ; l'écrire à la main évite une dépendance pour une seule signature, ce qui est la
/// discipline de ce projet.
pub trait ClientModele: Send + Sync {
    /// Écrit une réponse à partir du contexte.
    ///
    /// # Errors
    ///
    /// [`ErreurModele`] si le fournisseur ne répond pas, refuse, ou ne produit rien.
    fn repondre<'a>(
        &'a self,
        contexte: &'a ContexteConversation,
    ) -> Pin<Box<dyn Future<Output = Result<ReponseModele, ErreurModele>> + Send + 'a>>;

    /// Le nom du fournisseur, inscrit tel quel dans `consommation.fournisseur`.
    fn fournisseur(&self) -> &str;

    /// Le coût, en euros, d'un appel dont on connaît les unités consommées.
    ///
    /// Calculé ici plutôt qu'au point d'écriture : les tarifs sont une propriété du
    /// fournisseur, et deux fournisseurs ne facturent pas la même chose — l'un au jeton,
    /// l'autre à la seconde de GPU.
    fn cout_eur(&self, unites_entree: Option<i32>, unites_sortie: Option<i32>) -> rust_decimal::Decimal;
}
