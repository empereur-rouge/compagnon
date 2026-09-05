---
tags: [feature]
created: 2026-09-05
updated: 2026-09-05
version: v0.1.0
---

# Transport Telegram

## Résumé

Comment un message entre dans le service et comment une réponse en sort. C'est la seule
fonctionnalité complète de la phase 0, et le socle de toutes les suivantes : les phases 1 à 6
remplacent le **contenu** de la réponse, jamais le chemin qu'elle emprunte.

Le chemin complet :

```
Telegram ──POST /webhook──▶ Caddy (TLS) ──▶ Axum
                                              │  authentifier (secret partagé, temps constant)
                                              │  analyser (JSON → Update)
                                              │  extraire (Update → Recu, ou Ecart motivé)
                                              │  enfiler (file bornée, 256 places)
                                              ▼
                                          200 OK  ──────────▶ Telegram
                                              │
                                        (worker, hors requête)
                                              │  sendChatAction typing
                                              │  découper si > 4096 unités UTF-16
                                              ▼
                                          sendMessage ──────▶ Telegram
```

## Configuration

Tout vient de l'environnement, tout est validé au démarrage. Voir `.env.example`.

| Variable | Obligatoire | Rôle |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | oui | jeton `@BotFather`, forme `<chiffres>:<35 caractères>` |
| `TELEGRAM_SECRET_WEBHOOK` | oui | secret partagé, `[A-Za-z0-9_-]`, 32 à 256 caractères |
| `ADRESSE_ECOUTE` | non | défaut `0.0.0.0:8080` |
| `API_TELEGRAM` | non | défaut `https://api.telegram.org` ; sert aux tests et à un serveur Bot API local |
| `DOMAINE`, `COURRIEL_ACME` | oui (proxy) | certificat Let's Encrypt |

Déclarer le webhook après le premier démarrage :

```bash
docker compose exec bot compagnon webhook declarer https://$DOMAINE/webhook
```

## Modules et Fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `telegram` | `src/telegram/mod.rs` | client de l'API Bot, authentification du webhook |
| `telegram::types` | `src/telegram/types.rs` | formes reçues, extraction vers `Recu`, motifs d'`Ecart` |
| `telegram::envoi` | `src/telegram/envoi.rs` | enveloppe de réponse, erreurs, découpage |
| `webhook` | `src/webhook.rs` | réception, mise en file, contrat de statut |
| `worker` | `src/worker.rs` | consommation de la file, production de la réponse |
| `http` | `src/http.rs` | routeur, couches `tower`, état partagé, `/health` |
| `app` | `src/app.rs` | séquence de démarrage, extinction ordonnée, signaux |
| `config` | `src/config.rs` | lecture et validation de l'environnement |

## Fonctions Clés

| Fonction | Fichier | Description |
|---|---|---|
| `Canal::authentifier` | `src/telegram/mod.rs` | compare le secret présenté en temps constant |
| `egal_temps_constant` | `src/telegram/mod.rs` | comparaison sans divulgation par la durée |
| `Canal::envoyer_texte` | `src/telegram/mod.rs` | découpe puis envoie, un `sendMessage` par morceau |
| `Canal::appeler` | `src/telegram/mod.rs` | corps commun de tout appel ; **seul endroit** où une URL est construite |
| `Update::extraire` | `src/telegram/types.rs` | retient ce qui mérite réponse, ou dit pourquoi non |
| `envoi::decouper` | `src/telegram/envoi.rs` | découpe à 4096 unités UTF-16, coupe choisie |
| `envoi::point_de_coupe` | `src/telegram/envoi.rs` | recule au dernier `\n`, sinon à la dernière espace |
| `ErreurEnvoi::merite_une_reprise` | `src/telegram/envoi.rs` | sépare le transitoire du définitif |
| `webhook::recevoir` | `src/webhook.rs` | authentifie, analyse, enfile, acquitte |
| `worker::tourner` | `src/worker.rs` | consomme jusqu'à file fermée **et** vidée |
| `app::preparer` | `src/app.rs` | assemble et lie ; expose l'adresse effective |
| `Prepare::servir` | `src/app.rs` | sert, puis vide la file avant de rendre la main |

## Endpoints API

| Méthode | Path | Description |
|---|---|---|
| `POST` | `/webhook` | reçoit les mises à jour ; `401` si le secret ne correspond pas |
| `GET` | `/health` | sonde de santé : statut, version, occupation de la file |

Méthodes de l'API Bot appelées : `getMe` (au démarrage), `sendChatAction`, `sendMessage`,
`setWebhook`, `deleteWebhook`.

## Points durs, et ce qui les règle

**Le jeton est dans l'URL.** L'API Bot ne s'authentifie pas par en-tête. Toute URL est donc un
secret. `ErreurEnvoi` ne porte que le nom de la méthode, `Canal` et `EtatApp` ne dérivent pas
`Debug`, et `http::span_requete` ne trace ni la chaîne de requête ni les en-têtes. Un test
(`le_canal_ne_peut_pas_imprimer_son_jeton`) tient la moitié qui n'est pas tenue par le
compilateur.

**Le proxy journalise les en-têtes.** Caddy ne caviarde d'office que `Cookie`, `Set-Cookie`,
`Authorization` et `Proxy-Authorization`. `X-Telegram-Bot-Api-Secret-Token` n'en fait pas
partie : sans le filtre `delete` du `Caddyfile`, le secret s'écrit en clair dans les journaux du
proxy à chaque message. Une protection tenue d'un seul côté n'en est pas une.

**Telegram rejoue tout ce qui n'est pas `2xx`.** D'où le partage des statuts :

| Situation | Statut | Pourquoi |
|---|---|---|
| corps illisible | `200` | un rejeu ne le rendra pas lisible ; boucle de rejeu évitée |
| message écarté (groupe, bot, sans texte) | `200` | traité, et écarté sciemment |
| secret invalide | `401` | ne vient pas de Telegram |
| file pleine | `503` | mise à jour valide ; on **veut** le rejeu |

**4096 unités UTF-16, pas 4096 caractères.** Un emoji hors du plan de base vaut une `char` Rust
et deux unités UTF-16. Compter les caractères laisserait passer un message que Telegram
refuserait — et Telegram rejette le message entier, l'utilisateur ne voit rien.

**La file vit en mémoire.** L'extinction ordonnée la vide entièrement ; un `kill -9` perd son
contenu. Limite connue et bornée de la phase 0, levée par la file en base à bail de la phase 1.

## Interactions

- [[contrat-d-erreur]] — les codes que ce transport renvoie, et pourquoi ils sont vagues.
- `personnages`, `moteur-de-dialogue` (phase 1) — remplaceront `worker::repondre`, sans toucher
  au reste de ce chemin.
- `medias` (phase 3) — réutilisera `Canal::appeler` avec un corps multipart, et le cache de
  `file_id` s'appuiera sur le fait qu'un identifiant Telegram est réutilisable à vie.
