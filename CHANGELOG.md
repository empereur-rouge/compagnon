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
- **tests** : 25 tests unitaires, 12 de bout en bout et 1 doctest. Les tests e2e démarrent le
  vrai service sur une socket réelle et n'observent que ce que Telegram a reçu ; seule l'API
  Telegram est simulée, par `wiremock`, à la frontière HTTP sortante.
- **tests** : `tests/secrets.rs` regroupe les garanties d'absence — ce que le service ne doit
  pas faire. Cette classe de test se perd facilement : la version précédente du test de fuite
  couvrait exactement le complément du trou.
- **tests** : l'ordre « authentifier puis lire le corps » est éprouvé par un discriminant
  temporel — on annonce un corps qu'on n'envoie jamais, sur une socket brute. Une requête
  ordinaire ne distingue pas les deux ordres. Vérifié comme détectant bien la régression :
  3 s sans réponse sur l'ancien code, 239 µs sur le nouveau.
- **tests** : harnais réutilisable dans `tests/harnais/` — faux Telegram, service en marche sur
  port éphémère, attente d'appel avec compte-rendu d'échec. Les phases suivantes s'y greffent.

### Fixed

- **fix(telegram)** : **fuite du jeton du bot dans les journaux**. `ErreurEnvoi::Reseau` et
  `ErreurEnvoi::Illisible` conservaient un `reqwest::Error`, lequel transporte l'URL de l'appel
  et l'imprime dans son `Display` comme dans son `Debug` — or le jeton est un segment de cette
  URL. `worker::traiter` journalisant `%erreur` sur chaque envoi manqué, une simple coupure
  réseau écrivait le jeton dans `docker compose logs bot`, persisté sur disque. Les deux
  variantes ne portent plus qu'une `Panne` classée : le type n'a plus la capacité de porter une
  URL, y compris à travers un parcours de `source()`. Le test qui servait de filet construisait
  `ErreurEnvoi::Api`, seule variante qui ne pouvait pas fuir ; `tests/secrets.rs` éprouve
  désormais les trois, sur le vrai chemin contre un port fermé.
- **fix(http)** : le corps du webhook était **lu et alloué avant la vérification du secret**.
  Axum exécute les extracteurs — dont `Bytes`, qui draine la requête — puis seulement appelle le
  gestionnaire ; authentifier en première ligne du gestionnaire arrivait donc trop tard, et
  n'importe qui imposait la lecture de `TAILLE_MAX_CORPS` sur une adresse publique sans
  présenter de secret. La vérification vit dans une couche `route_layer` qui ne voit que les
  en-têtes, ce qui supprime au passage le clone intégral de la table d'en-têtes par requête.
  Conséquence assumée et testée : `GET /webhook` sans secret rend `401` et non `405`.
- **fix(telegram)** : `decouper` reparcourait tout le reste du texte à chaque tour, rendant la
  boucle quadratique — 1,19 s mesurées sur 4,5 millions d'unités UTF-16. Sans effet en phase 0,
  où l'entrant est plafonné, mais la sortie du modèle de la phase 1 ne le sera pas et le worker
  est à consommateur unique. Court-circuit en O(1) sur la longueur en octets.
- **fix(app)** : le vidage de la file à l'extinction était **non borné** alors que chaque
  `sendMessage` a un délai de 15 s : une file pleine face à un Telegram lent dépassait largement
  le sursis de Docker, et la perte était silencieuse. Vidage borné par `DELAI_VIDAGE` (25 s),
  aligné sur `stop_grace_period` (30 s), abandon journalisé en `ERROR`.

### Changed

- **change(error)** : une file pleine renvoie `5001` (`FileSaturee`) et non `9001` (`Interne`).
  Ce n'est pas une défaillance mais de la contre-pression, et le contrat public doit permettre
  de distinguer « le worker est saturé » de « le disque a lâché ». Statut `503` inchangé, pour
  que Telegram rejoue.
- **change(error)** : la couche d'enveloppe reconnaît une réponse déjà conforme à un marqueur
  privé au crate posé par `ApiError`, et non plus à son `content-type`. L'heuristique aurait été
  démentie en silence par un futur gestionnaire renvoyant son propre `Json(...)`.
- **change(http)** : `CatchPanicLayer` convertit une panique de gestionnaire en `9001` conforme.
  Sans lui, la connexion était coupée et Telegram voyait une réinitialisation au lieu d'un `5xx`.
- **change(telegram)** : `Ecart` porte son propre niveau de journal, comme `ErrorCode`. Le
  `match` de `webhook` finissait par un bras attrape-tout, qui aurait fait tomber en silence
  toute variante ajoutée par une phase suivante dans `debug!`. Le code d'erreur HTTP accolé à
  ces journaux est retiré : ces requêtes rendent `200`.
- **change(telegram)** : `Canal` et `Config` ne dérivent plus `Clone`, jamais utilisé et
  contradictoire — on interdit la copie du secret vers les journaux tout en autorisant la copie
  de la structure entière. `Recu` perd `Clone`, `PartialEq` et `Eq`, également inutilisés.
- **change(telegram)** : `Update::edited_message` est un `Option<IgnoredAny>` ; un
  `Option<Message>` faisait construire un message entier, allocations comprises, pour le jeter.
- **change(config)** : les validateurs ne rendent plus la valeur validée, ce qui faisait une
  seconde copie de chaque secret sur le tas.
- **change(http)** : `file_capacite` est lu sur le canal (`max_capacity`) et non recopié depuis
  la constante — les deux chiffres de la sonde viennent ainsi de la même source.

### Removed

- `horloge::instant`, `http::adresse_liee`, `EtatApp::new` : trois items publics sans appelant
  ou délégant d'une ligne. `main.rs` perd son ordre supérieur générique et ses trois closures.

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
