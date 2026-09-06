//! Gestes d'exploitation, exécutables sur l'artefact livré.
//!
//! # Pourquoi dans le binaire et pas dans des scripts
//!
//! Déclarer un webhook, vérifier qu'un conteneur est vivant : ces gestes se font au pire
//! moment — pendant un incident, sur une machine où l'arbre source n'est pas, sans chaîne de
//! compilation. Portés par le binaire, ils sont disponibles partout où le service l'est.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use crate::config::Config;
use crate::panne::Panne;
use crate::telegram::Canal;

/// Délai au-delà duquel la sonde considère le service comme muet.
///
/// Court volontairement : un `HEALTHCHECK` Docker qui met dix secondes à échouer retarde
/// d'autant la détection d'un conteneur bloqué.
const DELAI_SONDE: Duration = Duration::from_secs(3);

/// Ce qui a empêché une commande d'exploitation d'aboutir.
#[derive(Debug, thiserror::Error)]
pub enum ErreurCli {
    /// La configuration est illisible.
    #[error("{0}")]
    Config(#[from] crate::config::ErreurConfig),

    /// Le canal Telegram n'a pas pu être construit.
    #[error("{0}")]
    Canal(#[from] crate::telegram::ErreurCanal),

    /// Telegram a refusé l'appel.
    #[error("{0}")]
    Telegram(#[from] crate::telegram::envoi::ErreurEnvoi),

    /// Le service local n'a pas répondu à la sonde.
    ///
    /// Porte une [`Panne`] classée, pas la `reqwest::Error` : même discipline que partout
    /// ailleurs. L'adresse est reprise explicitement — elle est locale et sert au diagnostic,
    /// contrairement à l'URL que l'erreur de transport aurait transportée.
    #[error("le service ne répond pas sur {adresse} : {panne}")]
    Injoignable {
        /// L'adresse interrogée.
        adresse: String,
        /// La nature de l'échec.
        panne: Panne,
    },

    /// Le client HTTP de la sonde n'a pas pu être construit.
    ///
    /// Distincte de celle du canal Telegram, dont elle empruntait la variante : deux clients
    /// différents, deux causes différentes, et un exploitant qui doit savoir laquelle.
    #[error("client HTTP de la sonde inconstructible : {0}")]
    ClientSonde(Panne),

    /// Le service a répondu autre chose que `200`.
    #[error("le service répond {statut} sur /health")]
    MalPortant {
        /// Le statut renvoyé.
        statut: u16,
    },
}

/// Interroge `/health` et rend compte.
///
/// L'adresse d'écoute peut être `0.0.0.0` ; on interroge alors la boucle locale, seule adresse
/// dont on soit sûr qu'elle est joignable depuis l'intérieur du conteneur.
///
/// # Errors
///
/// Renvoie [`ErreurCli`] si le service ne répond pas, ou répond mal.
pub async fn sonde(config: &Config) -> Result<(), ErreurCli> {
    let adresse = adresse_locale(config.adresse_ecoute);
    let url = format!("http://{adresse}/health");

    let client = crate::panne::client_http(DELAI_SONDE, None).map_err(ErreurCli::ClientSonde)?;

    let reponse = client
        .get(&url)
        .send()
        .await
        .map_err(|erreur| ErreurCli::Injoignable {
            adresse: adresse.to_string(),
            panne: Panne::classer(&erreur),
        })?;

    let statut = reponse.status();
    let corps = reponse.text().await.unwrap_or_default();

    if statut.is_success() {
        println!("{corps}");
        Ok(())
    } else {
        eprintln!("{corps}");
        Err(ErreurCli::MalPortant {
            statut: statut.as_u16(),
        })
    }
}

/// Déclare l'adresse du webhook auprès de Telegram, avec le secret partagé.
///
/// # Errors
///
/// Renvoie [`ErreurCli`] si Telegram refuse l'adresse — le plus souvent parce qu'elle n'est pas
/// en HTTPS, ou que le certificat n'est pas encore émis.
pub async fn declarer_webhook(config: &Config, url: &str) -> Result<(), ErreurCli> {
    let canal = Canal::new(config)?;
    canal.declarer_webhook(url).await?;
    println!("webhook déclaré : {url}");
    println!(
        "secret partagé  : {} caractères",
        config.secret_webhook.longueur()
    );
    Ok(())
}

/// Retire le webhook. Telegram cesse d'appeler.
///
/// # Errors
///
/// Renvoie [`ErreurCli`] si l'appel échoue.
pub async fn retirer_webhook(config: &Config) -> Result<(), ErreurCli> {
    let canal = Canal::new(config)?;
    canal.retirer_webhook().await?;
    println!("webhook retiré");
    Ok(())
}

/// Traduit une adresse d'écoute en adresse joignable localement.
fn adresse_locale(ecoute: SocketAddr) -> SocketAddr {
    if ecoute.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), ecoute.port())
    } else {
        ecoute
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn une_ecoute_sur_toutes_les_interfaces_se_sonde_en_boucle_locale() {
        let cas = [
            ("0.0.0.0:8080", "127.0.0.1:8080"),
            ("[::]:8080", "127.0.0.1:8080"),
            ("127.0.0.1:9000", "127.0.0.1:9000"),
            ("192.168.1.10:8080", "192.168.1.10:8080"),
        ];
        for (ecoute, attendu) in cas {
            let source: SocketAddr = ecoute.parse().expect("adresse littérale");
            let obtenu = adresse_locale(source);
            println!("écoute {ecoute:20} -> sonde {obtenu}");
            assert_eq!(obtenu.to_string(), attendu);
        }
    }
}
