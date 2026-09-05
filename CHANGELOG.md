# Changelog

Toutes les modifications notables de ce projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/), et le projet applique
le [versionnage sémantique](https://semver.org/lang/fr/).

## [0.1.0] - Unreleased

Phase 0 — la boucle de transport, prouvée de bout en bout.

### Added

- **transport** : webhook Telegram sur Axum, authentifié par le secret partagé
  `X-Telegram-Bot-Api-Secret-Token`, comparé en temps constant. Les trois modes d'échec
  (en-tête absent, vide, erroné) partagent un code et un message publics indistinguables.
- **telegram** : client de l'API Bot écrit en direct sur `reqwest` — `getMe`, `sendMessage`,
  `sendChatAction`, `setWebhook`, `deleteWebhook`. Aucune URL n'apparaît dans une erreur ou un
  journal : le jeton du bot est un segment du chemin.
- **telegram** : découpage des messages sortants de plus de 4096 unités UTF-16, sous l'envoi
  plutôt qu'au-dessus, pour qu'aucun appelant ne puisse l'oublier. La coupe recule jusqu'au
  dernier saut de ligne, sinon la dernière espace, tant qu'elle ne sacrifie pas plus d'un quart
  du morceau.
- **worker** : file bornée à 256 places entre le webhook et la production des réponses. File
  pleine → `503` → Telegram rejoue : la contre-pression est déléguée à qui sait la gérer.
- **app** : extinction ordonnée — le serveur cesse de servir, le routeur est relâché, la file
  se vide, puis seulement le processus rend la main. Ce qui a été accusé est traité.
- **app** : `getMe` avant l'écoute. Un jeton révoqué ou mal collé fait échouer le **démarrage**,
  au lieu d'échouer silencieusement à chaque réponse.
- **config** : validation complète au démarrage, forme des secrets comprise. `Debug` masque le
  jeton et le secret de webhook.
- **error** : codes numériques stables `{"code": NNNN, "message": "..."}`, honorés aussi par les
  réponses du routeur et des couches `tower`. Numérotation partagée avec `agentbot` pour les
  codes communs.
- **exploitation** : `compagnon sonde`, `compagnon webhook declarer|retirer`, portés par le
  binaire livré — disponibles sur l'artefact, sans arbre source ni chaîne de compilation.
- **tests** : 24 tests unitaires et 7 de bout en bout. Ces derniers démarrent le vrai service
  sur une socket réelle et n'observent que ce que Telegram a reçu ; seule l'API Telegram est
  simulée, par `wiremock`, à la frontière HTTP sortante.
- **tests** : harnais réutilisable dans `tests/harnais/` — faux Telegram, service en marche sur
  port éphémère, attente d'appel avec compte-rendu d'échec. Les phases suivantes s'y greffent.

### Infrastructure

- **docker** : image à deux étages, exécution sans compilateur ni gestionnaire de paquets,
  utilisateur sans privilèges, `HEALTHCHECK` porté par le binaire.
- **proxy** : Caddy termine TLS — Telegram refuse un webhook qui n'est pas en HTTPS valide. Le
  journal du proxy **supprime** `X-Telegram-Bot-Api-Secret-Token` : Caddy ne caviarde d'office
  que `Cookie`, `Set-Cookie`, `Authorization` et `Proxy-Authorization`, et sans ce filtre le
  secret s'écrirait en clair à chaque message reçu.

### Notes

- La file vit en mémoire. L'extinction ordonnée la vide ; un arrêt brutal perd son contenu. La
  phase 1 la remplace par une file en base à bail.
- `tower-http` apparaît deux fois dans l'arbre (0.6 via `reqwest`, 0.7 en direct) : contrainte
  transitive, pas un choix.

## Version History

| Version | Date | Phase |
|---|---|---|
| 0.1.0 | non publiée | 0 — transport |
