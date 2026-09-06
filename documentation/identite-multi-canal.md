---
tags: [feature]
created: 2026-09-06
updated: 2026-09-06
version: v0.12.0
---

# Identité multi-canal — l'utilisateur cesse d'être son compte Telegram

## Résumé

`utilisateurs.id` est un **UUID interne**, et `identifiants_externes` fait le pont vers chaque
canal. La résolution `(canal, identifiant externe) → utilisateur` est le premier traitement de
toute requête entrante ; au-delà, plus rien dans le service ne connaît Telegram.

## Pourquoi, alors que Telegram est le seul canal

L'identifiant Telegram faisait une bonne clé, et l'argument tenait : stable, unique, connu dès le
premier message, il n'inventait pas une seconde identité. Ce qu'il ne permet pas, c'est qu'une
même personne existe sur deux canaux. Le jour où sortir de Telegram devient une décision plutôt
qu'une option, l'identifiant du canal cesse d'être une identité — il redevient ce qu'il est, une
**adresse**.

La bascule a été faite en phase 1.3 précisément parce que c'était la dernière fenêtre où elle ne
coûtait presque rien : la phase 1.5 ajoute l'onboarding, la 1.6 les abonnements et les quotas,
soit deux familles de tables de plus indexées par utilisateur, et de vraies données dedans.

## Ce que la migration a coûté, mesuré

| | |
|---|---|
| clés étrangères à basculer | 6 |
| index à recréer à l'identique | 8 |
| signatures Rust touchées | ~21 |
| lignes perdues | 0 |

`migrations/0009_identite_multi_canal.sql` préserve les données par jointure plutôt que de
repartir de zéro, et les tests de la phase 1.3 le vérifient après coup.

## Modules et fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `db::utilisateurs` | `src/db/utilisateurs.rs` | résolution, création, âge, pays |
| `admission` | `src/admission.rs` | résout **avant** d'enfiler : la file porte l'UUID |
| `telegram::types` | `src/telegram/types.rs` | `Recu::utilisateur_telegram` — une adresse, pas une identité |

## Fonctions clés

| Fonction | Fichier | Description |
|---|---|---|
| `utilisateurs::resoudre` | `src/db/utilisateurs.rs` | `(canal, identifiant) → UUID`, crée si inconnu |
| `utilisateurs::resoudre_telegram` | `src/db/utilisateurs.rs` | l'enveloppe du seul canal existant |

## Points durs, et ce qui les règle

**Deux premières requêtes simultanées.** Une lecture suivie d'une écriture ouvrirait une course
que l'index unique `(canal, identifiant_externe)` transformerait en erreur — sur le chemin
d'entrée d'un message, donc devant quelqu'un. `resoudre` insère avec
`on conflict … do update … returning`, ce qui rend l'appel idempotent : le perdant de la course
annule sa transaction et rend l'identité du gagnant.

**Le registre des coûts refuse la migration.** Le trigger de la migration 0007 n'admet qu'une
forme d'`update`, l'anonymisation RGPD. Remplir une colonne neuve n'en est pas une, et l'essai
l'a confirmé :

```
ERROR: consommation : la seule mise à jour admise est l'anonymisation RGPD
```

Le trigger est donc retiré puis reposé à l'identique autour du seul remplissage. Ce n'est pas un
contournement mais le régime que la migration 0006 a énoncé pour le texte éditorial : ce qui est
immuable l'est **hors migration**. Une migration se relit, se date et se versionne ; une console
`psql` non. Et le refus vaut démonstration que la garantie porte.

**L'horodatage d'audit.** Le trigger `mis_a_jour_le` de `personnages` est suspendu le temps du
remplissage : sans cela il dirait « migré le » au lieu de « dernière modification », et la
migration 0001 justifie ce trigger par « une colonne d'audit à laquelle on ne peut pas se fier ne
vaut rien ». Une bascule d'identité n'est pas une modification du compagnon. Vérifié : la valeur
est identique avant et après.

**Les tests continuent de parler en identifiants Telegram.** C'est ce qu'une mise à jour Telegram
contient, et ce que le harnais fabrique. Les faire manipuler des UUID les obligerait à connaître
la résolution — c'est-à-dire à contourner le chemin qu'ils éprouvent. `BaseDeTest::identite`
traduit, comme le service.

## Ce que ça n'a pas changé

Un seul compagnon par utilisateur, un seul fil, les quotas et la vérification d'âge par
utilisateur et non par canal. Le triangle utilisateur / compagnon / conversation reste fermé par
la même clé composite (`SCHEMA-NOYAU.md`, migration 0004), simplement portée par des UUID.

## Interactions

[[client-modele]] — le worker ne reçoit plus que l'UUID, via `file_messages`.
[[persistance]] — la file porte l'identité interne. [[transport-telegram]] — `Recu` reste le
porteur des adresses Telegram, et rien d'autre ne les lit.
