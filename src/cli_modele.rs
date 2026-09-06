//! `compagnon modele essai` — un appel réel au fournisseur, depuis l'artefact livré.
//!
//! # Pourquoi une commande et non un test
//!
//! Un test contre un faux serveur HTTP éprouve que le code sait lire *ce qu'on a supposé* que
//! le fournisseur renvoie. Il ne dit rien du fournisseur réel — de son en-tête
//! d'authentification, de la forme de son `usage`, du fait qu'il renvoie un identifiant de
//! modèle différent de celui demandé, ou qu'il coupe à `max_tokens` sans le dire.
//!
//! Cette commande fait l'appel pour de vrai et imprime tout ce qui en revient, y compris le
//! coût calculé au tarif configuré. C'est l'outil qui répond à « est-ce que ce fournisseur
//! marche, et combien coûte un message » — la question qu'on se pose avant de brancher un
//! nouvel hébergeur, et pendant l'incident où l'on soupçonne que c'est lui.
//!
//! Elle vit dans le binaire pour la même raison que [`crate::cli`] : sur la machine où on en a
//! besoin, l'arbre source n'est pas là.

use std::time::Duration;

use crate::modele::http::{ClientHttp, ConfigModele};
use crate::modele::{ClientModele, ContexteConversation, Role, Tour};

/// Le prompt système de l'essai.
///
/// Neutre à dessein : cette commande éprouve le **transport**, pas la personnalité. Un vrai
/// prompt de compagnon viendra de la base quand le worker appellera le modèle.
const PROMPT_ESSAI: &str = "Tu réponds brièvement, en français, en une ou deux phrases.";

/// Ce qui a empêché l'essai d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurEssai {
    /// La configuration du modèle est illisible.
    #[error("{0}")]
    Config(#[from] crate::config::ErreurConfig),

    /// Le client n'a pas pu être construit.
    #[error("{0}")]
    Construction(#[from] crate::modele::http::ErreurConstruction),

    /// Le fournisseur n'a pas répondu comme attendu.
    #[error("{0}")]
    Modele(#[from] crate::modele::ErreurModele),
}

/// Appelle le fournisseur configuré et rend compte de tout ce qui revient.
///
/// # Errors
///
/// [`ErreurEssai`] si la configuration est refusée, si le client ne se construit pas, ou si le
/// fournisseur échoue.
pub async fn essai(message: &str) -> Result<(), ErreurEssai> {
    let config = ConfigModele::depuis_environnement()?;
    println!("Configuration retenue :\n  {config:?}\n");

    let client = ClientHttp::new(config)?;
    let contexte = ContexteConversation {
        prompt_systeme: PROMPT_ESSAI.to_owned(),
        echanges: vec![Tour {
            role: Role::Utilisateur,
            texte: message.to_owned(),
        }],
    };

    println!("→ {message}");
    let resultat = client.repondre(&contexte).await;

    match resultat {
        Ok(reponse) => {
            let cout = client.cout_eur(reponse.unites_entree, reponse.unites_sortie);
            println!("← {}\n", reponse.texte);
            println!("  modèle rendu      : {}", reponse.modele);
            println!("  fournisseur       : {}", client.fournisseur());
            println!("  jetons entrée     : {}", affiche(reponse.unites_entree));
            println!("  jetons sortie     : {}", affiche(reponse.unites_sortie));
            println!("  durée mesurée     : {}", en_secondes(reponse.duree));
            println!("  coupée à max_tokens : {}", si_non(reponse.tronquee));
            // Six décimales, comme la colonne `consommation.cout_fournisseur_eur` : le coût
            // affiché doit être celui qui sera inscrit, pas un arrondi qui n'y correspond pas.
            println!("  coût               : {cout:.6} €");
            Ok(())
        }
        Err(erreur) => {
            println!("← échec : {erreur}");
            println!("  vaut une reprise : {}", si_non(erreur.merite_une_reprise()));
            Err(erreur.into())
        }
    }
}

/// Rend une unité, ou dit que le fournisseur ne l'a pas donnée.
fn affiche(unites: Option<i32>) -> String {
    unites.map_or_else(|| "non rendu par le fournisseur".to_owned(), |n| n.to_string())
}

/// Rend une durée en secondes, avec trois décimales.
fn en_secondes(duree: Duration) -> String {
    format!("{:.3} s", duree.as_secs_f64())
}

/// « oui » ou « non », plutôt que `true` / `false` devant quelqu'un qui lit un terminal.
const fn si_non(valeur: bool) -> &'static str {
    if valeur { "oui" } else { "non" }
}
