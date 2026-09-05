//! Un modèle de substitution, pour éprouver ce qui arrive quand le vrai ne répond pas.
//!
//! # Ce qu'il sert à prouver
//!
//! Les pannes d'un fournisseur de calcul — délai dépassé, débit saturé, génération vide — sont
//! rares, non reproductibles, et arrivent en production. Les seules occasions de vérifier que
//! le worker s'y comporte correctement sont donc **fabriquées**. Ce double les fabrique.
//!
//! Il retient aussi ce qu'on lui a demandé, ce qui permet de vérifier une propriété qu'aucun
//! test ne pourrait sinon atteindre : que le prompt système envoyé au modèle est bien celui
//! que la modération a validé, et pas une recomposition.
//!
//! # Le scénario
//!
//! Le double joue une suite d'[`Acte`], un par appel, puis **répète le dernier** indéfiniment.
//! C'est ce qui rend « échoue deux fois puis réussit » exprimable sans variante dédiée, et
//! c'est exactement le scénario dont la reprise bornée a besoin.

// Ce module n'est compilé que pour les tests, où un verrou empoisonné doit interrompre
// bruyamment plutôt que se propager en erreur.
#![allow(clippy::expect_used)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use rust_decimal::Decimal;

use super::{ClientModele, ContexteConversation, ErreurModele, ReponseModele};

/// Ce que le double fait d'un appel.
#[derive(Debug, Clone)]
pub enum Acte {
    /// Rendre ce texte.
    Repondre(String),
    /// Rendre ce texte après avoir attendu — pour éprouver un délai côté appelant.
    RepondreApres(String, Duration),
    /// Échouer de cette façon.
    Echouer(ErreurModeleClonable),
}

/// Une [`ErreurModele`] qu'on peut écrire dans un scénario.
///
/// [`ErreurModele`] n'est pas `Clone` — elle n'a aucune raison de l'être en production. Plutôt
/// que de l'affaiblir pour le confort des tests, le scénario porte cette description et la
/// convertit à l'appel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErreurModeleClonable {
    /// Voir [`ErreurModele::Injoignable`].
    Injoignable(super::Panne),
    /// Voir [`ErreurModele::Refuse`].
    Refuse(u16),
    /// Voir [`ErreurModele::Vide`].
    Vide,
}

impl From<ErreurModeleClonable> for ErreurModele {
    fn from(valeur: ErreurModeleClonable) -> Self {
        match valeur {
            ErreurModeleClonable::Injoignable(panne) => Self::Injoignable(panne),
            ErreurModeleClonable::Refuse(code) => Self::Refuse { code },
            ErreurModeleClonable::Vide => Self::Vide,
        }
    }
}

/// Ce que le double a observé.
#[derive(Debug, Default)]
struct Memoire {
    /// Les contextes reçus, dans l'ordre.
    recus: Vec<ContexteConversation>,
}

/// Un [`ClientModele`] qui joue un scénario écrit d'avance.
#[derive(Debug)]
pub struct ModeleDouble {
    actes: Vec<Acte>,
    memoire: Mutex<Memoire>,
}

impl ModeleDouble {
    /// Un double qui rend toujours le même texte.
    #[must_use]
    pub fn qui_repond(texte: &str) -> Self {
        Self::qui_joue(vec![Acte::Repondre(texte.to_owned())])
    }

    /// Un double qui échoue toujours de la même façon.
    #[must_use]
    pub fn qui_echoue(erreur: ErreurModeleClonable) -> Self {
        Self::qui_joue(vec![Acte::Echouer(erreur)])
    }

    /// Un double qui joue le scénario donné, puis en répète le dernier acte.
    ///
    /// # Panics
    ///
    /// Si le scénario est vide : un double sans acte n'a pas de comportement défini, et le
    /// découvrir à l'écriture du test vaut mieux que de le découvrir dans son résultat.
    #[must_use]
    pub fn qui_joue(actes: Vec<Acte>) -> Self {
        assert!(!actes.is_empty(), "un scénario de double doit avoir au moins un acte");
        Self { actes, memoire: Mutex::new(Memoire::default()) }
    }

    /// Combien de fois le double a été appelé.
    #[must_use]
    pub fn appels(&self) -> usize {
        self.memoire.lock().expect("verrou du double").recus.len()
    }

    /// Les contextes reçus, dans l'ordre d'appel.
    #[must_use]
    pub fn recus(&self) -> Vec<ContexteConversation> {
        self.memoire.lock().expect("verrou du double").recus.clone()
    }

    /// Le dernier contexte reçu, s'il y en a eu un.
    #[must_use]
    pub fn dernier_recu(&self) -> Option<ContexteConversation> {
        self.memoire.lock().expect("verrou du double").recus.last().cloned()
    }

    /// Enregistre l'appel et rend l'acte à jouer.
    ///
    /// Fonction séparée pour une raison mécanique : le verrou ne doit pas être tenu pendant un
    /// `await`, et [`Acte::RepondreApres`] en contient un.
    fn tour(&self, contexte: &ContexteConversation) -> Acte {
        let mut memoire = self.memoire.lock().expect("verrou du double");
        let rang = memoire.recus.len();
        memoire.recus.push(contexte.clone());
        drop(memoire);

        self.actes
            .get(rang)
            .or_else(|| self.actes.last())
            .cloned()
            .expect("le scénario n'est jamais vide, garanti à la construction")
    }
}

impl ClientModele for ModeleDouble {
    fn repondre<'a>(
        &'a self,
        contexte: &'a ContexteConversation,
    ) -> Pin<Box<dyn Future<Output = Result<ReponseModele, ErreurModele>> + Send + 'a>> {
        let acte = self.tour(contexte);
        Box::pin(async move {
            let (texte, duree) = match acte {
                Acte::Repondre(texte) => (texte, Duration::ZERO),
                Acte::RepondreApres(texte, attente) => {
                    tokio::time::sleep(attente).await;
                    (texte, attente)
                }
                Acte::Echouer(erreur) => return Err(erreur.into()),
            };

            // Une approximation grossière du découpage en jetons : le test n'a besoin que
            // d'un nombre qui varie avec la longueur, pas d'un vrai compte.
            let unites = |texte: &str| i32::try_from(texte.chars().count() / 4).unwrap_or(i32::MAX);

            Ok(ReponseModele {
                unites_entree: Some(unites(&contexte.prompt_systeme)),
                unites_sortie: Some(unites(&texte)),
                texte,
                modele: "double-de-test".to_owned(),
                duree,
            })
        })
    }

    fn fournisseur(&self) -> &str {
        "double"
    }

    /// Toujours gratuit : un double ne coûte rien, et un test qui vérifierait un tarif
    /// vérifierait celui du double.
    fn cout_eur(&self, _unites_entree: Option<i32>, _unites_sortie: Option<i32>) -> Decimal {
        Decimal::ZERO
    }
}

/// Un double dont chaque appel expire, quel que soit le scénario — raccourci du cas le plus
/// fréquent en test de reprise.
#[must_use]
pub fn modele_qui_expire() -> ModeleDouble {
    ModeleDouble::qui_echoue(ErreurModeleClonable::Injoignable(super::Panne::Delai))
}
