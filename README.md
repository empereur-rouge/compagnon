# compagnon

Assistant conversationnel personnel sur Telegram. Chaque utilisateur crée **le sien** — il le
nomme, définit sa personnalité, choisit son apparence et sa voix — et le service tient la
mémoire de cette relation, lui rendant une voix, un visage et une continuité. Un seul bot
Telegram sert tout le monde ; l'assistant, lui, appartient à chacun.

**État : phase 0.** La boucle de transport, et rien d'autre. Telegram appelle le webhook, le
service authentifie, extrait, met en file, répond en écho. Pas de base, pas de modèle, pas de
personnage — c'est délibéré : le transport est prouvé avant qu'une décision produit ne repose
dessus.

## Éprouver le bot en cinq minutes, sans rien déployer

Le webhook exige un domaine, un certificat valide et une machine joignable. Rien de tout cela
n'est nécessaire pour parler à son bot :

```bash
cp .env.example .env             # y coller le jeton donné par @BotFather
./scripts/base-de-test.sh        # un PostgreSQL jetable sur le port 5433
cargo run -- ecouter             # puis écrire au bot depuis Telegram
```

`ecouter` reçoit par **scrutation** (`getUpdates`) au lieu d'attendre un appel entrant : ni
domaine, ni TLS, ni tunnel, ni compte tiers. Ce n'est pas un raccourci de test — les messages
traversent la même admission, la même file et le même worker que la production. Seule la porte
d'entrée change.

## Mettre en service

```bash
cp .env.example .env          # jeton, secret de webhook, domaine, courriel ACME
docker compose up --detach --build
docker compose exec bot compagnon webhook declarer https://$DOMAINE/webhook
docker compose exec bot compagnon sonde
```

## Vérifier

```bash
./scripts/base-de-test.sh             # les tests exigent un vrai PostgreSQL
cargo test --release -- --nocapture   # 41 unitaires + 35 de bout en bout + 1 doctest
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
compagnon ecouter                  reçoit par scrutation — ni domaine, ni TLS, ni tunnel
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
| ✅ | codes d'erreur numériques stables, honorés jusque sur une panique de gestionnaire |
| ✅ | les deux secrets bannis des journaux, du proxy, et **du type des erreurs** |
| ✅ | authentification du webhook **avant** que le corps ne soit lu |
| ✅ | réception par scrutation, pour éprouver le bot depuis un poste de travail |
| ✅ | file **en base à bail** : rien n'est perdu à un arrêt brutal |
| ✅ | quatre consommateurs concurrents, ordre tenu **dans** chaque conversation |
| ✅ | vérification d'âge exigée avant tout accès au moteur |
| ✅ | le prompt système est **composé**, jamais saisi — la sûreté est structurelle |
| ✅ | un compagnon ne peut pas s'activer sans être passé par la modération |
| ⬜ | moteur de dialogue, parcours de création dans Telegram — phase 1.3+ |
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
segment du chemin. Toute URL est donc un secret. Interdire aux erreurs d'en porter une n'a pas
suffi tant que c'était une règle à tenir : `ErreurEnvoi` conservait un `reqwest::Error`, dont
le `Display` imprime l'URL, et une coupure réseau écrivait le jeton dans les journaux. Le type
n'a plus la **capacité** d'en porter une. Le corollaire vaut aussi pour le proxy, voir le
[Caddyfile](Caddyfile).

**L'authentification est une couche, pas une première ligne.** Axum lit le corps avant
d'appeler le gestionnaire : authentifier dedans laissait n'importe qui imposer la lecture de
256 Kio sans présenter de secret. Conséquence assumée : `GET /webhook` rend `401`, pas `405`.

**Tête-à-tête uniquement.** Les messages de groupe sont écartés : répondre dans un groupe
exposerait à tous une conversation intime et changerait la nature du produit.

**Le webhook ne répond jamais lui-même.** Il authentifie, enfile, acquitte. Produire une
réponse appartient au worker — sans quoi la phase 1, qui appellera un modèle pendant plusieurs
secondes, ferait rejouer Telegram à chaque message.

## Documentation

| | |
|---|---|
| [Carte du projet](documentation/MOC.md) | par où commencer |
| [Un assistant par personne](documentation/un-assistant-par-personne.md) | le modèle produit, et ce qu'il décide |
| [Persistance](documentation/persistance.md) | base, file à bail, concurrence |
| [Le compagnon](documentation/compagnon.md) | catalogues, prompt composé, modération |
| [Un seul bot](documentation/un-seul-bot.md) | pourquoi un seul bot Telegram |
| [Transport Telegram](documentation/transport-telegram.md) | la fiche de la phase 0 |
| [journey-map.html](journey-map.html) | les parcours utilisateur et le code qui les sert |
| [CHANGELOG.md](CHANGELOG.md) | ce qui a changé, version par version |
