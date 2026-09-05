---
tags: [reference]
created: 2026-09-05
updated: 2026-09-05
version: v0.2.2
---

# Un assistant par personne, qui lui appartient

## La décision

Chaque utilisateur crée **son** assistant à l'inscription, et n'en a qu'un. Il le nomme, définit
sa personnalité, choisit son apparence et sa voix, et peut le modifier ensuite. Il n'existe pas
de catalogue de personnages partagés : personne ne « choisit Sophie », chacun a le sien.

C'est le modèle Replika, et non celui de character.ai. La différence n'est pas cosmétique —
elle décide du schéma, du coût, de la modération et de l'onboarding.

## Pourquoi ce modèle plutôt que le catalogue

Ce que l'utilisateur vient chercher n'est pas de la variété, c'est une relation. Toute la
mémoire converge alors sur un seul lien, qui s'approfondit au lieu de se disperser sur cinq
personnages tièdes. L'onboarding n'a qu'une création à faire réussir, la modération qu'une
fiche à examiner par personne, et l'abonnement a un objet évident : *cet* assistant-là.

Un fil de discussion Telegram est par ailleurs une interface pauvre pour un sélecteur permanent.
Un assistant unique n'en demande aucun.

## Le schéma

```
utilisateur (clé : chat_id Telegram)
   │
   └── assistant                    lui appartient — 1 pour 1
         ├── fiche                  nom, personnalité, ton, limites — éditable par lui
         ├── ancre                  image de référence + prompt d'apparence + seed, FIGÉS
         ├── voix                   timbre choisi
         │
         └── relation               ce que l'assistant sait de lui
               ├── fenetre          les derniers tours, mot pour mot
               ├── journal          résumé roulant de ce qui s'est passé entre eux
               ├── souvenirs        faits extraits : prénom, métier, ce qu'il a confié
               └── etat             humeur, familiarité, dernière interaction
```

La relation est **structurellement** privée : elle pend de l'assistant, qui pend de
l'utilisateur. Aucune requête ne peut la traverser latéralement. Sur un produit intime, une
fuite de mémoire entre deux comptes n'est pas un bug dont on se relève — cela se conçoit au
premier `CREATE TABLE`, pas après.

La cardinalité 1:1 est tenue par une contrainte d'unicité et non par une règle applicative. Si
un jour plusieurs assistants deviennent une option payante, seule cette contrainte tombe.

## Quatre conséquences, à connaître avant d'écrire la phase 1

**Le cache d'invite change d'économie.** Avec un catalogue partagé, un préfixe caché servait des
milliers d'utilisateurs. Ici chacun a son propre espace de cache, qui ne mord que si *cette*
personne discute assez pour le garder chaud. C'est une différence de coût du simple au multiple,
et elle se mesure sur du trafic réel avant de choisir le moteur — elle peut à elle seule
décider du modèle retenu.

**La modération change d'échelle, et c'est le plus lourd.** Chaque utilisateur écrit une fiche :
la surface à examiner passe d'une poignée de créateurs à la totalité de la base. À ce volume,
une revue humaine est impossible. C'est un classifieur au moment de la création, avec refus dur
— et c'est là que vit le contrôle non négociable : aucune apparence mineure, jamais, ni dans la
fiche, ni dans l'ancre, ni dans une génération ultérieure.

**L'onboarding *est* la création.** `/start` n'est pas un accueil, c'est un parcours : nommer,
définir, choisir une apparence. C'est le moment le plus fragile du produit — qui abandonne en
cours de création ne revient pas — et le premier point de modération. Il mérite d'être conçu
avant le dialogue lui-même.

**L'ancre d'identité se confirme.** Une LoRA entraînée par assistant est hors de question quand
chaque inscription en crée un. C'est image de référence + IPAdapter/InstantID + seed figée,
produite une fois à la création et immuable ensuite. Le prompt d'apparence est figé au même
moment : l'utilisateur ne le réécrit pas librement à chaque image, sinon la modération de la
création ne vaut plus rien.

## Ce que ce modèle supprime du produit

- pas de catalogue, pas de navigation, pas de recherche de personnages
- pas de sélecteur d'assistant dans le fil
- pas de liens profonds par personnage — `?start=` reste libre pour du parrainage
- pas de fiche publique, donc pas de modération de contenu *exposé à des tiers* : ce qu'un
  utilisateur écrit dans sa fiche ne s'affiche que pour lui

## Ce que ce modèle aggrave

L'assistant est censé être *le sien*, mais l'icône dans la liste des discussions Telegram reste
celle de la plateforme — voir [[un-seul-bot]]. Le décalage est plus sensible ici que dans un
catalogue partagé. Les atténuations restent les mêmes, et elles comptent davantage : envoyer le
portrait de l'assistant dès la fin de la création, et lui faire porter son nom dans le fil.

## Ordre de la phase 1

1. le schéma et l'isolation, avec leurs contraintes
2. la modération à la création — avant tout ce qui produit du contenu
3. le parcours de création
4. le dialogue
5. la mémoire

## Interactions

- [[un-seul-bot]] — pourquoi un seul bot Telegram, et ce que Telegram interdit.
- [[transport-telegram]] — le `chat_id` qui identifie une personne, et les deux portes d'entrée.
