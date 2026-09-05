# compagnon

Plateforme de personnages conversationnels sur Telegram. Un bot unique, derrière lequel vivent
plusieurs personnages écrits par les utilisateurs ; le service tient la mémoire de chaque
relation et rend au personnage une voix, un visage et une continuité.

**État : phase 0.** La boucle de transport, et rien d'autre. Telegram appelle le webhook, le
service authentifie, extrait, met en file, répond en écho. Pas de base, pas de modèle, pas de
personnage — c'est délibéré : le transport est prouvé avant qu'une décision produit ne repose
dessus.

## Démarrer

```bash
cp .env.example .env          # jeton @BotFather, secret de webhook, domaine
docker compose up --detach --build
docker compose exec bot compagnon webhook declarer https://$DOMAINE/webhook
docker compose exec bot compagnon sonde
```

En développement, sans Docker :

```bash
cargo run                     # lit .env, sert sur ADRESSE_ECOUTE
cargo run -- sonde
```

## Vérifier

```bash
cargo test --release -- --nocapture   # 24 tests unitaires + 7 de bout en bout
cargo clippy --all-targets            # zéro avertissement attendu
cargo doc --no-deps --open            # zéro avertissement attendu
```

Les tests de bout en bout démarrent le **vrai** service sur une socket réelle et n'observent
que ce que Telegram a reçu. Un seul élément est simulé — l'API Telegram, par `wiremock`, à la
frontière HTTP sortante. Le harnais vit dans [`tests/harnais/`](tests/harnais/mod.rs) et les
phases suivantes s'y greffent : c'est lui qu'on étend, pas qu'on contourne.

## Exploiter

```
compagnon                          sert le webhook et fait tourner le worker
compagnon sonde                    interroge /health, sort en 0 ou 1 (HEALTHCHECK)
compagnon webhook declarer <url>   déclare l'adresse du webhook auprès de Telegram
compagnon webhook retirer          retire le webhook
```

## Ce qui est vrai aujourd'hui, et ce qui ne l'est pas encore

| | |
|---|---|
| ✅ | webhook authentifié par secret partagé, en temps constant |
| ✅ | découpage des réponses de plus de 4096 unités UTF-16 |
| ✅ | file bornée à contre-pression : file pleine → `503` → Telegram rejoue |
| ✅ | extinction ordonnée sans perte de ce qui a été accusé |
| ✅ | codes d'erreur numériques stables, secrets bannis des journaux et du proxy |
| ⬜ | base, personnages, moteur de dialogue — phase 1 |
| ⬜ | mémoire : journal roulant, souvenirs, état de relation — phase 2 |
| ⬜ | photos, audio, vidéo — phases 3 à 6 |

**Limite connue de la phase 0** : la file vit en mémoire. Une extinction *ordonnée* la vide
entièrement ; un `kill -9` ou une panne de courant perd ce qu'elle contient. La phase 1 la
remplace par une file en base à bail, sur le modèle de celle d'`agentbot`.

## Décisions structurantes

**API Bot en direct, pas de bibliothèque cliente.** `teloxide` apporte un répartiteur et une
machine à états de dialogue qu'un service piloté par modèle n'utilise pas, contre plusieurs
centaines de dépendances transitives. L'API Bot est du JSON sur HTTPS.

**Le jeton est dans l'URL.** L'API Bot ne s'authentifie pas par en-tête : le jeton est un
segment du chemin. Toute URL est donc un secret — aucune n'atteint un journal, une erreur ou un
`Debug`. Le corollaire vaut aussi pour le proxy, voir le [Caddyfile](Caddyfile).

**Tête-à-tête uniquement.** Les messages de groupe sont écartés : répondre dans un groupe
exposerait à tous une conversation intime et changerait la nature du produit.

**Le webhook ne répond jamais lui-même.** Il authentifie, enfile, acquitte. Produire une
réponse appartient au worker — sans quoi la phase 1, qui appellera un modèle pendant plusieurs
secondes, ferait rejouer Telegram à chaque message.

## Documentation

| | |
|---|---|
| [Carte du projet](documentation/MOC.md) | par où commencer |
| [Transport Telegram](documentation/transport-telegram.md) | la fiche de la phase 0 |
| [journey-map.html](journey-map.html) | les parcours utilisateur et le code qui les sert |
| [CHANGELOG.md](CHANGELOG.md) | ce qui a changé, version par version |
