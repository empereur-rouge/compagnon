//! La nature d'une panne de transport, sans rien de ce qui l'a causée.
//!
//! # Pourquoi ce type existe
//!
//! Une `reqwest::Error` **transporte l'URL de l'appel**, et l'imprime dans son `Display` comme
//! dans son `Debug`. Conservée dans une variante d'erreur, elle finit dans un journal — c'est
//! arrivé dans ce projet, sur le chemin de l'API Telegram, dont l'URL contient le jeton du bot.
//!
//! Le module d'envoi affirmait « aucune URL n'atteint un journal » pendant que `worker::traiter`
//! écrivait `%erreur` sur chaque envoi manqué, jeton compris, dans des journaux que
//! `compose.yaml` persiste sur disque.
//!
//! Le correctif n'a pas été d'ajouter un `without_url()` sur les sites d'appel — `reqwest`
//! l'offre, et le crate documente lui-même pourquoi. Ç'aurait été remettre la garantie dans la
//! discipline, là où elle avait déjà échoué. Il a été de retirer au type d'erreur la
//! **capacité** de porter une URL : le compilateur garantit alors qu'aucun `Display`, `Debug`
//! ou parcours de `source()` — y compris celui d'[`crate::error::ApiError::diagnostic`] — ne
//! peut en atteindre une.
//!
//! Ce qu'on perd : le détail de la cause système (« connection refused » plutôt que
//! « connexion impossible »). Ce qu'on garde : de quoi décider s'il faut réessayer, ce qui est
//! la seule chose dont le code se sert.
//!
//! # Pourquoi il vit au niveau de la caisse
//!
//! Parce que le problème n'est ni telegram ni modèle : c'est celui de tout appel sortant. Deux
//! copies du même enum ont coexisté le temps d'un commit, et deux copies d'une garantie
//! divergent — celle qu'on oublie de corriger devient celle par laquelle la fuite revient.

/// Ce qui a empêché un appel HTTP d'aboutir.
///
/// Ne porte **aucune** donnée : pas d'URL, pas d'hôte, pas de message du fournisseur. C'est
/// la propriété qui fait tout l'intérêt du type, et elle est tenue par l'absence de champs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panne {
    /// L'appel n'a pas abouti dans le délai imparti.
    Delai,
    /// La connexion n'a pas pu être établie : DNS, refus, TLS.
    Connexion,
    /// La réponse est arrivée mais n'a pas pu être lue.
    Corps,
    /// La requête elle-même n'a pas pu être formée ou émise.
    Requete,
    /// Rien de ce qui précède.
    Autre,
}

impl Panne {
    /// Classe une erreur `reqwest`, sans en retenir autre chose que sa nature.
    ///
    /// Prend la référence et ne la conserve pas : c'est ce qui garantit que l'URL ne survit
    /// pas à l'appel.
    #[must_use]
    pub fn classer(erreur: &reqwest::Error) -> Self {
        if erreur.is_timeout() {
            Self::Delai
        } else if erreur.is_connect() {
            Self::Connexion
        } else if erreur.is_decode() || erreur.is_body() {
            Self::Corps
        } else if erreur.is_request() {
            Self::Requete
        } else {
            Self::Autre
        }
    }

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

    /// Vrai si la même requête, rejouée plus tard, a une chance d'aboutir.
    ///
    /// Toutes les pannes de transport le méritent : aucune n'a atteint l'état distant. C'est
    /// une méthode et non une constante parce que l'appelant raisonne sur des pannes sans
    /// avoir à savoir qu'elles sont toutes équivalentes de ce point de vue — et parce que
    /// [`Panne::Requete`] cessera de l'être le jour où elle couvrira une requête malformée.
    #[must_use]
    pub const fn merite_une_reprise(self) -> bool {
        true
    }
}

impl std::fmt::Display for Panne {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.libelle())
    }
}
