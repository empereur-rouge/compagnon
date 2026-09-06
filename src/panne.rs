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
    /// La réponse a commencé d'arriver et son corps s'est interrompu.
    ///
    /// À distinguer d'un corps **lisible mais non conforme** : celui-là n'est pas une panne de
    /// transport, et il ne se rejoue pas de la même façon. Voir
    /// [`crate::modele::ErreurModele::ReponseIllisible`].
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
            Self::Corps => "réponse interrompue",
            Self::Requete => "requête non émise",
            Self::Autre => "cause indéterminée",
        }
    }

}

/// Vrai si un code de réponse HTTP mérite qu'on rejoue la même requête.
///
/// # Pourquoi ici, et pas dans chaque appelant
///
/// Cette table était écrite deux fois — une pour Telegram, une pour le modèle — et c'est la
/// partie qui **change** : un fournisseur ajoute un `529`, un autre veut `408`. Le module
/// [`crate::panne`] a été créé pour la classification du transport en disant que « le problème
/// n'est ni telegram ni modèle : c'est celui de tout appel sortant ». La décision de reprise
/// sur un code de réponse est la même propriété transversale ; elle s'était arrêtée à mi-chemin.
///
/// `429` : débit dépassé, il faut attendre. `5xx` : défaillance côté serveur.
/// Le reste — `400` requête mal formée, `401`/`403` identifiants refusés, `404` route
/// inexistante — refera exactement la même erreur, et rejouer ne fait qu'épuiser les tentatives
/// en retardant le moment où quelqu'un apprend que ça ne marche pas.
#[must_use]
pub const fn reprise_pour_statut(code: u16) -> bool {
    matches!(code, 429 | 500..=599)
}

impl std::fmt::Display for Panne {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.libelle())
    }
}

/// Construit un client HTTP sortant, ou rend la nature de l'échec.
///
/// # Pourquoi un constructeur partagé
///
/// Trois endroits bâtissaient un `reqwest::Client`, avec trois traitements de l'échec : le
/// modèle classait en [`Panne`], le canal Telegram gardait la `reqwest::Error`, et la sonde
/// `/health` **empruntait la variante d'erreur du canal Telegram** pour rapporter l'échec de
/// son propre client. Un exploitant lisant « client HTTP du canal Telegram inconstructible »
/// pendant un incident de sonde locale cherchait au mauvais endroit.
///
/// Le module affirme, quinze lignes plus haut, que le correctif « a été de retirer au type
/// d'erreur la **capacité** de porter une URL ». Deux types la conservaient. Une discipline
/// perd sa force dès qu'elle admet des exceptions.
///
/// # Errors
///
/// [`Panne`] si la pile HTTP ne peut pas être bâtie — en pratique, une pile TLS indisponible.
pub fn client_http(
    delai: std::time::Duration,
    connexion: Option<std::time::Duration>,
) -> Result<reqwest::Client, Panne> {
    let mut constructeur = reqwest::Client::builder().timeout(delai);
    if let Some(connexion) = connexion {
        constructeur = constructeur.connect_timeout(connexion);
    }
    constructeur.build().map_err(|erreur| Panne::classer(&erreur))
}
