//! L'implémentation concrète : un appel HTTP à une API compatible OpenAI.
//!
//! # Pourquoi cette convention précisément
//!
//! `POST /chat/completions` n'est pas retenu par attachement à un fournisseur, mais parce que
//! c'est la seule forme que **vLLM, TGI, et la quasi-totalité des hébergeurs de GPU exposent
//! déjà**. C'est ce qui rend le remplacement du backend réel : passer d'un serverless facturé
//! au jeton à un GPU dédié qu'on opère soi-même ne change qu'une URL et un tarif. Sans elle, le
//! trait [`ClientModele`] ne serait qu'une indirection.
//!
//! # Ce qui n'est pas ici, et pourquoi
//!
//! **Pas de boucle de reprise.** La file à bail en a déjà une : bornée par `tentatives_max`,
//! persistante, et qui ne retient aucun worker pendant l'attente. Une seconde boucle à
//! l'intérieur de l'appel multiplierait les deux — trois tentatives de file × trois d'appel
//! font neuf appels facturés pour un incident — et retarderait d'autant le moment où
//! l'utilisateur apprend que ça ne marche pas. Ce module fait **un** appel, avec un délai
//! explicite, et rend une erreur que l'appelant sait classer par
//! [`ErreurModele::merite_une_reprise`].
//!
//! # Sur la clé
//!
//! Contrairement à l'API Telegram, l'authentification passe par un en-tête `Authorization` et
//! non par un segment d'URL. L'URL n'est donc pas secrète ici — mais la classification en
//! [`Panne`] est appliquée quand même : elle ne coûte rien, et c'est une discipline qui perd sa
//! force dès qu'elle admet des exceptions selon le fournisseur du jour.
//!
//! # Une limite connue, à traiter en phase 1.8
//!
//! Les modèles à raisonnement séparent leur réflexion du texte final. Ce serveur-ci la met
//! dans un champ `reasoning_content` que ce module ignore — c'est le bon comportement. Mais
//! **tous ne le font pas** : certains l'inscrivent dans `content`, entre balises `<think>`. Le
//! compagnon enverrait alors sa réflexion à l'utilisateur, ce qui détruirait l'illusion que
//! tout le produit tient. Le filtrage de la sortie appartient aux garde-fous de la phase 1.8 ;
//! il est signalé ici pour qu'il ne soit pas découvert en production.
//!
//! Le corps d'une réponse d'erreur n'est **jamais** lu. Un fournisseur qui refuse reprend
//! souvent la requête dans son message — donc le prompt système, donc tout ce que le compagnon
//! est. Seul le code de statut est retenu.

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{ClientModele, ContexteConversation, ErreurModele, Panne, ReponseModele};
use crate::config::{ErreurConfig, lire, lire_ou};
use crate::secret::Secret;

/// Délai au-delà duquel l'appel est abandonné, quand `MODELE_DELAI_S` n'est pas positionnée.
///
/// Soixante secondes : un modèle de vingt-quatre milliards de paramètres qui écrit cinq cents
/// jetons met une dizaine de secondes en régime normal, et davantage sur un démarrage à froid
/// de serverless. Plus court couperait des appels qui allaient aboutir ; beaucoup plus long
/// laisserait quelqu'un devant un « en train d'écrire… » qui ne mène nulle part.
const DELAI_DEFAUT_S: u64 = 60;

/// Nombre de jetons de sortie par défaut.
///
/// Un message de compagnon est court. Cinq cents jetons valent environ deux mille caractères,
/// bien en deçà de la limite de Telegram, et au-delà de ce qu'on veut lire dans une
/// conversation.
const JETONS_MAX_DEFAUT: &str = "500";

/// Température par défaut.
///
/// Haute à dessein : un compagnon qui répond deux fois la même chose à deux jours d'intervalle
/// détruit l'illusion de continuité que tout le produit cherche à tenir.
const TEMPERATURE_DEFAUT: &str = "0.85";

/// Un million — le dénominateur des tarifs affichés par les fournisseurs.
const PAR_MILLION: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0);

/// Ce dont l'appel au fournisseur a besoin.
///
/// Lue dans l'environnement et validée d'un coup, comme [`crate::config::Config`] : un tarif
/// mal orthographié découvert au premier message produit un registre de coûts faux, et un
/// registre de coûts faux est pire qu'absent — il répond à la question du prix, avec un chiffre
/// inventé.
///
/// `Debug` est **dérivé** : `cle` est un [`Secret`], donc le rendu la masque. Une implémentation
/// manuscrite listait les neuf champs et reproduisait exactement le dérivé — sauf qu'un dixième
/// champ ajouté en aurait été silencieusement absent, donc absent du journal de démarrage et de
/// la sortie de `compagnon modele essai`, l'outil dont toute la raison d'être est de montrer la
/// configuration réelle.
#[derive(Debug)]
pub struct ConfigModele {
    /// Racine de l'API, sans barre oblique finale — par exemple `https://api.exemple.com/v1`.
    pub base: String,
    /// La clé d'accès.
    pub cle: Secret,
    /// L'identifiant du modèle demandé.
    pub modele: String,
    /// Le nom de l'hébergeur, inscrit tel quel dans `consommation.fournisseur`.
    pub fournisseur: String,
    /// Jetons de sortie maximum.
    pub jetons_max: u32,
    /// Température d'échantillonnage.
    pub temperature: f32,
    /// Délai au-delà duquel l'appel est abandonné.
    pub delai: Duration,
    /// Prix, en euros, d'un million de jetons d'entrée.
    pub prix_entree_eur_par_million: Decimal,
    /// Prix, en euros, d'un million de jetons de sortie.
    pub prix_sortie_eur_par_million: Decimal,
}

impl ConfigModele {
    /// Lit et valide la configuration du modèle depuis l'environnement.
    ///
    /// # Errors
    ///
    /// [`ErreurConfig`] à la première variable absente ou mal formée, en la nommant.
    pub fn depuis_environnement() -> Result<Self, ErreurConfig> {
        let base = lire("MODELE_API_BASE")?;
        let base = base.trim_end_matches('/').to_owned();
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err(ErreurConfig::Invalide {
                variable: "MODELE_API_BASE",
                raison: "doit commencer par http:// ou https://".to_owned(),
            });
        }

        let cle = lire("MODELE_API_CLE")?;
        let modele = lire("MODELE_NOM")?;
        let fournisseur = lire("MODELE_FOURNISSEUR")?;

        let jetons_max = nombre::<u32>("MODELE_JETONS_MAX", JETONS_MAX_DEFAUT)?;
        if jetons_max == 0 {
            return Err(ErreurConfig::Invalide {
                variable: "MODELE_JETONS_MAX",
                raison: "zéro jeton de sortie : le modèle ne pourrait rien écrire".to_owned(),
            });
        }

        let temperature = nombre::<f32>("MODELE_TEMPERATURE", TEMPERATURE_DEFAUT)?;
        if !(0.0..=2.0).contains(&temperature) {
            return Err(ErreurConfig::Invalide {
                variable: "MODELE_TEMPERATURE",
                raison: "doit être entre 0 et 2".to_owned(),
            });
        }

        let delai = Duration::from_secs(nombre::<u64>(
            "MODELE_DELAI_S",
            &DELAI_DEFAUT_S.to_string(),
        )?);

        // Les deux tarifs sont obligatoires, sans valeur par défaut. Un défaut à zéro ferait
        // dire au registre que le service ne coûte rien — la réponse est fausse, et personne
        // n'a de raison d'aller la vérifier.
        let prix_entree_eur_par_million = tarif("MODELE_PRIX_ENTREE_EUR_PAR_MILLION")?;
        let prix_sortie_eur_par_million = tarif("MODELE_PRIX_SORTIE_EUR_PAR_MILLION")?;

        Ok(Self {
            base,
            cle: Secret::nouveau(cle),
            modele,
            fournisseur,
            jetons_max,
            temperature,
            delai,
            prix_entree_eur_par_million,
            prix_sortie_eur_par_million,
        })
    }
}

/// Lit une variable numérique facultative, avec sa valeur par défaut.
fn nombre<T: FromStr>(nom: &'static str, defaut: &str) -> Result<T, ErreurConfig> {
    lire_ou(nom, defaut)
        .parse::<T>()
        .map_err(|_| ErreurConfig::Invalide {
            variable: nom,
            raison: "nombre illisible".to_owned(),
        })
}

/// Lit un tarif obligatoire, refusé s'il est négatif.
fn tarif(nom: &'static str) -> Result<Decimal, ErreurConfig> {
    let brut = lire(nom)?;
    let valeur = Decimal::from_str(&brut).map_err(|_| ErreurConfig::Invalide {
        variable: nom,
        raison: "tarif illisible, attendu un nombre décimal comme « 0.18 »".to_owned(),
    })?;
    if valeur.is_sign_negative() {
        return Err(ErreurConfig::Invalide {
            variable: nom,
            raison: "un tarif négatif ferait dire au registre que le service rapporte".to_owned(),
        });
    }
    Ok(valeur)
}

// ---------------------------------------------------------------------------
// Le format de fil, côté requête
// ---------------------------------------------------------------------------

/// Un message tel que la convention OpenAI l'attend.
#[derive(Serialize)]
struct MessageApi<'a> {
    role: &'static str,
    content: &'a str,
}

/// Le corps de `POST /chat/completions`.
#[derive(Serialize)]
struct RequeteApi<'a> {
    model: &'a str,
    messages: Vec<MessageApi<'a>>,
    max_tokens: u32,
    temperature: f32,
}

// ---------------------------------------------------------------------------
// Le format de fil, côté réponse
// ---------------------------------------------------------------------------

/// La réponse du fournisseur.
///
/// Tous les champs sont facultatifs sauf `choices`, et c'est délibéré : les hébergeurs
/// s'écartent de la convention sur les marges. Un `usage` absent doit dégrader la mesure de
/// coût, pas faire échouer une réponse que l'utilisateur attend.
#[derive(Deserialize)]
struct ReponseApi {
    /// Présent quand le fournisseur annonce une erreur **sans** l'exprimer par le statut HTTP.
    ///
    /// Ce n'est pas une hypothèse : un vrai serveur compatible OpenAI rend `200 OK` avec
    /// `{"error": "Unexpected endpoint or method."}` sur un chemin inexistant. Sans ce champ,
    /// la réponse se lit comme une génération vide — donc comme un incident passager, donc
    /// rejouable, alors que c'est une URL fausse qui ne guérira pas.
    ///
    /// Typé `serde_json::Value` parce que les fournisseurs y mettent tantôt une chaîne, tantôt
    /// un objet `{message, type, code}`. Seule sa **présence** est utilisée ; son contenu n'est
    /// jamais transporté dans une erreur.
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChoixApi>,
    #[serde(default)]
    usage: Option<UsageApi>,
}

/// Une génération candidate. Une seule est demandée.
#[derive(Deserialize)]
struct ChoixApi {
    #[serde(default)]
    message: Option<MessageRecu>,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Le contenu d'une génération.
#[derive(Deserialize)]
struct MessageRecu {
    #[serde(default)]
    content: Option<String>,
}

/// Le décompte des jetons, quand le fournisseur le rend.
#[derive(Deserialize)]
struct UsageApi {
    #[serde(default)]
    prompt_tokens: Option<i32>,
    #[serde(default)]
    completion_tokens: Option<i32>,
}

// ---------------------------------------------------------------------------
// Le client
// ---------------------------------------------------------------------------

/// Un [`ClientModele`] qui parle à une API compatible OpenAI.
///
/// `Debug` est dérivé : la clé est un [`Secret`], donc le rendu la masque.
#[derive(Debug)]
pub struct ClientHttp {
    client: reqwest::Client,
    config: ConfigModele,
    /// `<base>/chat/completions`, composée une fois plutôt qu'à chaque appel.
    url: String,
}

/// Ce qui a empêché le client d'être construit.
#[derive(Debug, thiserror::Error)]
pub enum ErreurConstruction {
    /// Le client HTTP n'a pas pu être bâti — en pratique, une pile TLS indisponible.
    ///
    /// Ne porte pas l'erreur `reqwest` : même discipline que partout ailleurs.
    #[error("client HTTP du modèle impossible à construire : {0}")]
    Client(Panne),
}

impl ClientHttp {
    /// Construit le client à partir de la configuration validée.
    ///
    /// # Errors
    ///
    /// [`ErreurConstruction::Client`] si la pile HTTP ne peut pas être bâtie.
    pub fn new(config: ConfigModele) -> Result<Self, ErreurConstruction> {
        // Le délai porte sur l'appel entier, pas seulement sur la connexion : c'est la
        // génération qui est lente, et c'est elle qu'il faut pouvoir abandonner.
        let client = crate::panne::client_http(config.delai, None)
            .map_err(ErreurConstruction::Client)?;

        let url = format!("{}/chat/completions", config.base);
        Ok(Self { client, config, url })
    }

    /// La configuration en vigueur, pour les journaux de démarrage.
    #[must_use]
    pub const fn config(&self) -> &ConfigModele {
        &self.config
    }

    /// L'appel, écrit une fois et enveloppé par [`ClientModele::repondre`].
    async fn appeler(&self, contexte: &ContexteConversation) -> Result<ReponseModele, ErreurModele> {
        let mut messages = Vec::with_capacity(contexte.echanges.len() + 1);
        messages.push(MessageApi {
            role: "system",
            content: &contexte.prompt_systeme,
        });
        for tour in &contexte.echanges {
            messages.push(MessageApi {
                role: tour.role.dans_l_api(),
                content: &tour.texte,
            });
        }

        let corps = RequeteApi {
            model: &self.config.modele,
            messages,
            max_tokens: self.config.jetons_max,
            temperature: self.config.temperature,
        };

        let debut = Instant::now();
        let reponse = self
            .client
            .post(&self.url)
            .bearer_auth(self.config.cle.exposer())
            .json(&corps)
            .send()
            .await
            .map_err(|erreur| ErreurModele::Injoignable(Panne::classer(&erreur)))?;

        let code = reponse.status();
        if !code.is_success() {
            // Le corps n'est pas lu : il reprendrait la requête, donc le prompt système.
            return Err(ErreurModele::Refuse {
                code: code.as_u16(),
            });
        }

        let charge: ReponseApi = reponse.json().await.map_err(|erreur| {
            // Deux échecs très différents sous une même API. `is_decode()` : le corps est
            // arrivé entier et ne ressemble pas à ce que la convention prévoit — une racine
            // d'API fausse, un fournisseur qui ne suit pas la convention. Rejouer refera
            // exactement la même chose, en facturant à chaque fois. Tout le reste est une
            // interruption de transport, qui se rejoue.
            if erreur.is_decode() {
                ErreurModele::ReponseIllisible
            } else {
                ErreurModele::Injoignable(Panne::classer(&erreur))
            }
        })?;
        let duree = debut.elapsed();

        if charge.error.is_some() {
            // Le contenu n'est pas remonté — il reprendrait la requête — mais il est
            // journalisé ici, où il sert au diagnostic sans traverser un type d'erreur.
            tracing::warn!(
                fournisseur = %self.config.fournisseur,
                "le fournisseur a répondu 200 en annonçant une erreur"
            );
            return Err(ErreurModele::RefusApplicatif);
        }

        let Some(choix) = charge.choices.into_iter().next() else {
            return Err(ErreurModele::Vide);
        };
        let tronquee = choix.finish_reason.as_deref() == Some("length");
        let texte = choix
            .message
            .and_then(|message| message.content)
            .unwrap_or_default();
        // Un texte fait uniquement d'espaces est un vide qui ne se voit pas : Telegram
        // refuserait le message, et l'utilisateur n'aurait pas de réponse sans qu'aucune
        // erreur ne soit remontée. La raison de l'arrêt distingue les deux causes, qui
        // n'envoient pas l'exploitant au même endroit.
        if texte.trim().is_empty() {
            return Err(if tronquee {
                ErreurModele::Tronquee
            } else {
                ErreurModele::Vide
            });
        }

        let (unites_entree, unites_sortie) = charge
            .usage
            .map_or((None, None), |usage| {
                (usage.prompt_tokens, usage.completion_tokens)
            });

        Ok(ReponseModele {
            texte,
            // L'identifiant rendu par le fournisseur prime sur celui demandé : c'est celui qui
            // a réellement écrit, et c'est sur lui que se comparent les coûts.
            modele: charge.model.unwrap_or_else(|| self.config.modele.clone()),
            unites_entree,
            unites_sortie,
            duree,
            tronquee,
        })
    }
}

impl ClientModele for ClientHttp {
    fn repondre<'a>(
        &'a self,
        contexte: &'a ContexteConversation,
    ) -> Pin<Box<dyn Future<Output = Result<ReponseModele, ErreurModele>> + Send + 'a>> {
        Box::pin(self.appeler(contexte))
    }

    fn fournisseur(&self) -> &str {
        &self.config.fournisseur
    }

    /// Le coût au tarif configuré.
    ///
    /// Une unité inconnue vaut zéro plutôt que d'annuler le calcul : un fournisseur qui ne rend
    /// pas `usage` doit produire un coût sous-estimé et visible, pas un trou dans le registre.
    /// Le sous-total qui manque se lit alors dans l'écart entre le registre et la facture.
    fn cout_eur(&self, unites_entree: Option<i32>, unites_sortie: Option<i32>) -> Decimal {
        let part = |unites: Option<i32>, prix: Decimal| {
            Decimal::from(unites.unwrap_or(0)) * prix / PAR_MILLION
        };
        part(unites_entree, self.config.prix_entree_eur_par_million)
            + part(unites_sortie, self.config.prix_sortie_eur_par_million)
    }
}
