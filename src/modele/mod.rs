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
//! variante d'erreur ne peut porter l'URL de l'appel** : [`ErreurModele::Injoignable`] ne porte
//! qu'une [`Panne`] — un énuméré nu — et [`ErreurModele::Refuse`] qu'un `u16`. Le corps d'une
//! réponse d'erreur n'entre nulle part, parce qu'un message d'erreur de fournisseur reprend
//! souvent la requête, donc le prompt système, donc tout ce que le compagnon est.

#[cfg(any(test, feature = "fixtures"))]
pub mod double;
pub mod http;

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

impl Role {
    /// Le nom du rôle dans l'API du fournisseur.
    ///
    /// `user` et `assistant` sont les valeurs de la convention OpenAI, que vLLM, TGI et la
    /// plupart des hébergeurs exposent. Les traduire ici garde le reste du code dans le
    /// vocabulaire du produit, et laisse un fournisseur à la convention différente n'avoir
    /// qu'un endroit à changer.
    #[must_use]
    pub const fn dans_l_api(self) -> &'static str {
        match self {
            Self::Utilisateur => "user",
            Self::Compagnon => "assistant",
        }
    }
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
    /// Vrai si la génération a été coupée par la limite de jetons plutôt que terminée.
    ///
    /// Un compagnon dont la phrase s'arrête au milieu d'un mot se lit comme une panne. C'est
    /// une propriété du produit, pas un détail de transport : l'appelant en a besoin pour
    /// décider quoi envoyer, et le fournisseur est le seul à la connaître.
    pub tronquee: bool,
}

// La nature de la panne est celle de tout appel sortant : réexportée depuis `crate::panne`,
// pas redéfinie ici. Une seconde copie de ce type a existé le temps d'un commit, et deux
// copies d'une garantie divergent.
pub use crate::panne::Panne;

/// Ce qui a empêché le modèle de répondre.
///
/// `Clone` est dérivé pour que les scénarios de test portent **cette** énumération plutôt qu'une
/// copie parallèle. Une copie manuscrite avait existé le temps d'un commit, et elle avait déjà
/// oublié deux variantes — précisément les deux ajoutées après mesure sur un vrai serveur, dont
/// la seule non rejouable. Toutes les données portées sont `Copy` : la dérivation ne coûte rien.
#[derive(Debug, Clone, thiserror::Error)]
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

    /// La réponse est arrivée entière et ne ressemble pas à ce que la convention prévoit.
    ///
    /// Distincte de [`ErreurModele::Injoignable`] avec [`Panne::Corps`], qui est un corps
    /// **interrompu** : celui-là se rejoue, celui-ci non. Un serveur qui rend du JSON non
    /// conforme en rendra encore au coup suivant — c'est une racine d'API fausse ou un
    /// fournisseur qui ne suit pas la convention, pas un incident.
    ///
    /// La distinction n'est pas théorique : sans elle, une `MODELE_API_BASE` erronée et un
    /// incident réseau reçoivent le même verdict, et le premier consomme trois générations
    /// facturées avant d'être abandonné.
    #[error("le fournisseur a rendu une réponse non conforme")]
    ReponseIllisible,

    /// Le fournisseur a répondu `200`, et a annoncé une erreur dans le corps.
    ///
    /// Constaté sur un vrai serveur : `POST /v9/chat/completions` (chemin inexistant) rend
    /// **`200 OK`** avec `{"error": "Unexpected endpoint or method."}`. Un faux serveur aurait
    /// rendu `404`, et la classification aurait paru correcte sans l'être.
    ///
    /// Distinguée de [`ErreurModele::Vide`] parce que la conséquence diffère : un vide se
    /// rejoue, une erreur applicative se rejouera à l'identique. Les confondre transforme une
    /// URL mal saisie en trois tentatives silencieuses et un message « le modèle n'a rien
    /// produit » devant quelqu'un qui cherche une panne de modèle.
    ///
    /// Ne porte pas le texte de l'erreur : un message de fournisseur reprend souvent la
    /// requête. Il est journalisé au point d'appel, jamais transporté.
    #[error("le fournisseur a répondu 200 en annonçant une erreur")]
    RefusApplicatif,

    /// Le modèle a répondu, mais sans rien dire.
    ///
    /// Un filtre côté fournisseur, une génération refusée en silence. Distinguée d'un refus
    /// parce qu'elle se rejoue utilement.
    #[error("le modèle n'a rien produit")]
    Vide,

    /// La limite de jetons a coupé la génération **avant** le moindre texte.
    ///
    /// Mesuré sur un vrai modèle : un modèle à raisonnement dépense son budget dans sa
    /// réflexion et rend `content: ""` avec `finish_reason: "length"`. Sur cinq appels
    /// identiques à `max_tokens = 80`, quatre finissent ainsi.
    ///
    /// Séparée de [`ErreurModele::Vide`] pour le **diagnostic**, pas pour la décision : les
    /// deux se rejouent, mais « le modèle n'a rien produit » enverrait chercher une panne de
    /// modèle là où il faut augmenter `MODELE_JETONS_MAX` ou changer de modèle. C'est la même
    /// confusion que [`ErreurModele::RefusApplicatif`] corrige côté transport — une cause
    /// permanente déguisée en incident passager.
    #[error("la limite de jetons a coupé la génération avant tout texte")]
    Tronquee,
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
            // Aucun échange n'a abouti : l'état distant est inchangé, et la cause est
            // passagère par nature — DNS, connexion refusée, délai, corps interrompu.
            Self::Injoignable(_) => true,
            // Le fournisseur a répondu, mais sans texte exploitable. Rejouable : mesuré, un
            // appel sur cinq aboutit là où les quatre autres ont épuisé leur budget. Rejouer
            // sert donc l'utilisateur qui attend, pendant que le libellé distinct sert
            // l'exploitant qui lit le journal.
            Self::Vide | Self::Tronquee => true,
            // Le fournisseur a été joint et n'a pas rendu ce qu'il devait. Rien n'indique que
            // la même requête aboutirait plus tard, et la cause la plus probable est une
            // configuration fausse — qu'aucune reprise ne corrigera.
            Self::RefusApplicatif | Self::ReponseIllisible => false,
            Self::Refuse { code } => crate::panne::reprise_pour_statut(*code),
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
