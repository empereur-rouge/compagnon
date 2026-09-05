//! Harnais partagé des tests de bout en bout.
//!
//! # Ce qui est simulé, et ce qui ne l'est pas
//!
//! Un seul élément est faux : **l'API Telegram**, remplacée par un serveur `wiremock` à la
//! frontière HTTP sortante. Tout le reste est le vrai chemin — la vraie séquence de démarrage
//! ([`compagnon::app::preparer`]), le vrai routeur avec ses couches `tower`, la vraie
//! authentification, la vraie file, le vrai worker, et de vraies requêtes HTTP sur une vraie
//! socket. Un test qui appellerait le gestionnaire en direct testerait le gestionnaire ; celui-ci
//! teste le service.
//!
//! # Pourquoi le port zéro
//!
//! Lier sur `127.0.0.1:0` laisse le système choisir un port libre. Deux tests peuvent alors
//! tourner en parallèle sans se disputer une adresse, et aucun n'échoue parce qu'un service
//! traîne sur 8080.
//!
//! # Ce que ce harnais devra devenir
//!
//! Les phases suivantes y ajouteront des façades : le fournisseur de modèle (phase 1), le
//! moteur d'images (phase 5). Elles se montent ici, à côté de [`FauxTelegram`], pour que chaque
//! test reste une conversation lisible plutôt qu'un montage.

#![allow(dead_code, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use compagnon::app::{self, Prepare};
use compagnon::config::Config;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Le jeton employé par les tests. De la bonne forme, sans correspondance réelle.
pub const JETON: &str = "123456789:AAExempleDeJetonQuiNeSertAAbsolumen";

/// Le secret de webhook employé par les tests.
pub const SECRET: &str = "un-secret-de-quarante-huit-caracteres-exactement";

/// Au-delà, un appel attendu est considéré comme n'étant jamais venu.
///
/// Généreux à dessein : le but n'est pas de mesurer une latence, mais de ne pas rendre le test
/// dépendant de la charge de la machine qui l'exécute.
const DELAI_ATTENTE: Duration = Duration::from_secs(5);

/// Intervalle entre deux consultations du journal d'appels.
const PAS_ATTENTE: Duration = Duration::from_millis(10);

/// L'API Telegram, simulée.
pub struct FauxTelegram {
    serveur: MockServer,
}

impl FauxTelegram {
    /// Démarre le faux Telegram, avec les réponses nominales des quatre méthodes de la phase 0.
    pub async fn demarrer() -> Self {
        let serveur = MockServer::start().await;

        Self::monter(
            &serveur,
            "getMe",
            json!({"ok": true, "result": {
                "id": 123_456_789, "is_bot": true,
                "first_name": "Compagnon", "username": "compagnon_de_test_bot"
            }}),
        )
        .await;

        Self::monter(
            &serveur,
            "sendMessage",
            json!({"ok": true, "result": {"message_id": 1000}}),
        )
        .await;

        Self::monter(
            &serveur,
            "sendChatAction",
            json!({"ok": true, "result": true}),
        )
        .await;
        Self::monter(&serveur, "setWebhook", json!({"ok": true, "result": true})).await;
        Self::monter(
            &serveur,
            "deleteWebhook",
            json!({"ok": true, "result": true}),
        )
        .await;

        Self { serveur }
    }

    async fn monter(serveur: &MockServer, methode: &str, corps: Value) {
        Mock::given(method("POST"))
            .and(path(format!("/bot{JETON}/{methode}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(corps))
            .mount(serveur)
            .await;
    }

    /// La configuration qui pointe le service vers ce faux Telegram.
    ///
    /// `adresse_ecoute` est sur le port zéro : l'adresse réelle n'est connue qu'après liaison.
    pub fn config(&self) -> Config {
        Config {
            jeton_bot: JETON.to_owned(),
            secret_webhook: SECRET.to_owned(),
            adresse_ecoute: "127.0.0.1:0".parse().expect("adresse littérale"),
            api_telegram: self.serveur.uri(),
        }
    }

    /// Tous les appels reçus sur une méthode de l'API Bot, dans l'ordre.
    pub async fn appels(&self, methode: &str) -> Vec<Request> {
        let attendu = format!("/bot{JETON}/{methode}");
        self.serveur
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|requete| requete.url.path() == attendu)
            .collect()
    }

    /// Les corps JSON des appels reçus sur une méthode.
    pub async fn corps(&self, methode: &str) -> Vec<Value> {
        self.appels(methode)
            .await
            .iter()
            .map(|requete| {
                serde_json::from_slice(&requete.body).expect("le service envoie du JSON valide")
            })
            .collect()
    }

    /// Attend qu'une méthode ait été appelée au moins `combien` fois.
    ///
    /// Renvoie les corps reçus. Échoue avec un compte-rendu de ce qui *a* été appelé — sans
    /// cela, un test qui expire ne dit pas si rien n'est parti ou si autre chose est parti.
    pub async fn attendre(&self, methode: &str, combien: usize) -> Vec<Value> {
        let debut = std::time::Instant::now();
        loop {
            let corps = self.corps(methode).await;
            if corps.len() >= combien {
                return corps;
            }
            if debut.elapsed() > DELAI_ATTENTE {
                let journal = self.journal().await;
                panic!(
                    "« {methode} » attendu {combien} fois, reçu {} en {:?}.\nAppels observés :\n{journal}",
                    corps.len(),
                    debut.elapsed()
                );
            }
            tokio::time::sleep(PAS_ATTENTE).await;
        }
    }

    /// Un compte-rendu lisible de tout ce que le service a envoyé à Telegram.
    pub async fn journal(&self) -> String {
        let requetes = self.serveur.received_requests().await.unwrap_or_default();
        if requetes.is_empty() {
            return "  (aucun appel)".to_owned();
        }
        requetes
            .iter()
            .map(|requete| {
                // Le chemin porte le jeton : on ne garde que la méthode, comme le fait le
                // service lui-même dans ses journaux.
                let methode = requete.url.path().rsplit('/').next().unwrap_or("?");
                let corps = String::from_utf8_lossy(&requete.body);
                format!("  {methode:<16} {corps}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Un service en marche, joignable, qu'on peut éteindre proprement.
pub struct EnMarche {
    /// L'adresse réellement obtenue.
    pub adresse: std::net::SocketAddr,
    client: reqwest::Client,
    arret: oneshot::Sender<()>,
    tache: JoinHandle<()>,
}

/// Démarre le service contre un faux Telegram, et rend de quoi lui parler.
pub async fn demarrer(faux: &FauxTelegram) -> EnMarche {
    let config = faux.config();
    let prepare: Prepare = app::preparer(&config)
        .await
        .expect("le service doit démarrer contre le faux Telegram");

    // Lue avant que `servir` ne consomme la structure : l'adresse vient du port éphémère que
    // le système a choisi, et c'est la seule façon de la connaître.
    let adresse_servie = prepare.adresse;

    let (arret, reception) = oneshot::channel();
    let tache = tokio::spawn(async move {
        prepare
            .servir(async move {
                let _ = reception.await;
            })
            .await
            .expect("le service ne doit pas s'interrompre sur une erreur");
    });

    // Attendre que la socket accepte, plutôt que de dormir une durée arbitraire.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client de test");
    attendre_ecoute(&client, adresse_servie).await;

    EnMarche {
        adresse: adresse_servie,
        client,
        arret,
        tache,
    }
}

async fn attendre_ecoute(client: &reqwest::Client, adresse: std::net::SocketAddr) {
    let url = format!("http://{adresse}/health");
    let debut = std::time::Instant::now();
    while debut.elapsed() < DELAI_ATTENTE {
        if client.get(&url).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(PAS_ATTENTE).await;
    }
    panic!("le service n'écoute toujours pas sur {adresse} après {DELAI_ATTENTE:?}");
}

/// Longueur d'un texte en unités UTF-16, mesurée par **le code du service**.
///
/// Réexportée plutôt que recopiée : un test qui vérifie une limite avec sa propre mesure ne
/// teste pas le code qu'il croit tester — si la mesure du service était fausse, il passerait.
// `allow` : chaque cible de test n'utilise qu'une partie du harnais.
#[allow(unused_imports)]
pub use compagnon::telegram::envoi::longueur_utf16;

impl EnMarche {
    /// Poste une mise à jour sur le webhook, avec le bon secret.
    pub async fn poster(&self, update: &Value) -> reqwest::Response {
        self.poster_avec_secret(update, SECRET).await
    }

    /// Poste une mise à jour avec le secret de son choix.
    pub async fn poster_avec_secret(&self, update: &Value, secret: &str) -> reqwest::Response {
        self.client
            .post(format!("http://{}/webhook", self.adresse))
            .header("X-Telegram-Bot-Api-Secret-Token", secret)
            .json(update)
            .send()
            .await
            .expect("le webhook doit répondre")
    }

    /// Poste un corps brut, pour les cas où ce n'est pas du JSON valide.
    pub async fn poster_brut(&self, corps: &'static str) -> reqwest::Response {
        self.client
            .post(format!("http://{}/webhook", self.adresse))
            .header("X-Telegram-Bot-Api-Secret-Token", SECRET)
            .header("Content-Type", "application/json")
            .body(corps)
            .send()
            .await
            .expect("le webhook doit répondre")
    }

    /// Interroge la sonde de santé.
    pub async fn sante(&self) -> Value {
        self.obtenir("/health")
            .await
            .json()
            .await
            .expect("la sonde renvoie du JSON")
    }

    /// Poste un corps volumineux avec le secret de son choix.
    pub async fn poster_volumineux(&self, octets: usize, secret: &str) -> reqwest::Response {
        self.client
            .post(format!("http://{}/webhook", self.adresse))
            .header("X-Telegram-Bot-Api-Secret-Token", secret)
            .header("Content-Type", "application/json")
            .body("x".repeat(octets))
            .send()
            .await
            .expect("le webhook doit répondre")
    }

    /// Annonce un corps, puis n'en envoie **rien**, et rapporte ce que le service répond.
    ///
    /// C'est le seul moyen d'observer *quand* le corps est lu. Une requête ordinaire ne
    /// discrimine pas les deux ordres — le refus est le même — alors qu'ici :
    ///
    /// - si le service lit le corps avant d'authentifier, il attend un corps qui n'arrive
    ///   jamais et ne répond qu'au bout de son délai de requête ;
    /// - s'il authentifie d'abord, il répond immédiatement.
    ///
    /// Passe par une socket brute : un client HTTP normal n'accepte pas d'annoncer un corps
    /// qu'il ne fournit pas. Renvoie la ligne de statut et la durée d'attente.
    pub async fn annoncer_un_corps_sans_l_envoyer(
        &self,
        octets: usize,
        secret: &str,
        patience: Duration,
    ) -> (Option<String>, Duration) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let debut = std::time::Instant::now();
        let mut socket = tokio::net::TcpStream::connect(self.adresse)
            .await
            .expect("le service doit accepter la connexion");

        let entetes = format!(
            "POST /webhook HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             X-Telegram-Bot-Api-Secret-Token: {secret}\r\nContent-Length: {octets}\r\n\r\n",
            self.adresse
        );
        socket
            .write_all(entetes.as_bytes())
            .await
            .expect("les en-têtes doivent partir");
        socket.flush().await.expect("vidage de la socket");

        // Et rien de plus : le corps annoncé n'arrivera jamais.
        let mut tampon = vec![0_u8; 256];
        let lu = tokio::time::timeout(patience, socket.read(&mut tampon)).await;
        let ecoule = debut.elapsed();

        let statut = match lu {
            Ok(Ok(n)) if n > 0 => String::from_utf8_lossy(&tampon[..n])
                .lines()
                .next()
                .map(str::to_owned),
            _ => None,
        };
        (statut, ecoule)
    }

    /// Un `POST` sans corps sur un chemin quelconque, pour éprouver le routage de méthode.
    pub async fn poster_sur(&self, chemin: &str) -> reqwest::Response {
        self.client
            .post(format!("http://{}{chemin}", self.adresse))
            .send()
            .await
            .expect("le service doit répondre")
    }

    /// Un `GET` sur un chemin quelconque, pour éprouver le contrat d'erreur.
    pub async fn obtenir(&self, chemin: &str) -> reqwest::Response {
        self.client
            .get(format!("http://{}{chemin}", self.adresse))
            .send()
            .await
            .expect("le service doit répondre")
    }

    /// Éteint le service et attend que la file soit vidée.
    ///
    /// Rendre la main ici signifie que tout ce qui avait été accepté a été traité.
    ///
    /// Prend `self` par valeur, et non `&mut self` : c'est ce qui fait d'un second appel une
    /// erreur de compilation plutôt qu'un no-op silencieux, et cela supprime les deux `Option`
    /// dont le seul rôle était de rendre le vidage réentrant.
    pub async fn eteindre(self) {
        let _ = self.arret.send(());
        self.tache
            .await
            .expect("le service doit s'éteindre proprement");
    }
}

/// Une mise à jour privée ordinaire, telle que Telegram l'envoie.
pub fn update_privee(update_id: i64, texte: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "from": {
                "id": 42, "is_bot": false, "first_name": "Erwan",
                "username": "erwan", "language_code": "fr"
            },
            "chat": {"id": 42, "first_name": "Erwan", "type": "private"},
            "date": 1_760_000_000_i64,
            "text": texte
        }
    })
}
