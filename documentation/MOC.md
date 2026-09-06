---
tags: [moc]
created: 2026-09-05
updated: 2026-09-06
version: v0.10.0
---

# Carte du projet — compagnon

Plateforme de personnages conversationnels sur Telegram. Cette carte est l'index : chaque
fonctionnalité a sa fiche, chaque fiche nomme le code qui la porte.

## Par où commencer

1. [[transport-telegram]] — comment un message entre et comment une réponse sort. Tout le
   reste se branche dessus.
2. [[un-assistant-par-personne]] — le modèle produit. Chacun crée **son** assistant et n'en a
   qu'un ; il n'y a pas de catalogue partagé. Décide du schéma, du coût et de la modération.
3. [[un-seul-bot]] — un seul bot Telegram pour toute la plateforme, et ce que Telegram
   interdit. À lire avec la précédente.
4. [[contrat-d-erreur]] — les codes numériques stables, et pourquoi les messages publics sont
   vagues sur certaines tranches.
5. `README.md` à la racine — démarrer, vérifier, exploiter.

## Fiches

| Fiche | Phase | Sujet |
|---|---|---|
| [[client-modele]] | 1.3 | trait, appel HTTP, double de test, registre des coûts |
| [[compagnon]] | 1.2 | catalogues, traits, prompt composé, modération |
| [[persistance]] | 1.1 | base, file à bail, workers concurrents, vérification d'âge |
| [[transport-telegram]] | 0 | webhook **et scrutation**, authentification, file, envoi, découpage |
| [[contrat-d-erreur]] | 0 | codes numériques, messages publics, journalisation |
| [[un-seul-bot]] | 0 | pourquoi un seul bot Telegram, et ce que Telegram interdit |
| [[un-assistant-par-personne]] | 0 | le modèle produit : un assistant possédé, pas un catalogue |

## Ce qui viendra

Chaque phase ajoute sa fiche ici. Les intitulés sont fixés d'avance pour que les liens
`[[...]]` écrits en avance pointent un jour quelque part.

| Fiche à venir | Phase | Sujet |
|---|---|---|
| `memoire` | 2 | journal roulant, souvenirs structurés, état de relation |
| `medias` | 3 | file de génération à bail, cache de `file_id` |
| `voix` | 4 | synthèse sortante, transcription entrante, transcodage Opus |
| `imagier` | 5 | ancre d'identité, génération à la demande |

## Décisions transverses

- **Le jeton Telegram est dans l'URL** — donc aucune URL n'atteint un journal, une erreur ou un
  `Debug`. Vaut aussi pour le proxy : voir la section correspondante de [[transport-telegram]].
- **Tête-à-tête uniquement** — les messages de groupe sont écartés à l'extraction.
- **Un assistant par personne, qui lui appartient** — pas de catalogue partagé. Voir
  [[un-assistant-par-personne]].
- **Un seul bot Telegram pour tous** — l'utilisateur possède son assistant, pas un bot. Voir
  [[un-seul-bot]] : Telegram n'offre d'ailleurs aucune API de création de bot.
- **L'utilisateur ne tape jamais le prompt système** — il choisit dans des catalogues clos, le
  service compose. Si aucune valeur possible n'évoque un mineur, aucune composition ne le peut :
  la sûreté est une propriété de l'ensemble des valeurs, pas un filtre. Voir [[compagnon]].
- **La file survit au processus** — table à bail plutôt que canal en mémoire ; ce qui n'a pas
  été traité est repris au démarrage suivant. Voir [[persistance]].
- **La concurrence est donnée par la base, pas par le worker** — la requête de prise écarte
  tout utilisateur déjà servi, donc l'ordre tient dans une conversation sans qu'aucun code Rust
  ne synchronise quoi que ce soit.
- **Deux portes, un seul chemin** — webhook en production, scrutation sur un poste de travail ;
  tout ce qui suit l'admission est rigoureusement identique, et testé comme tel.
- **Le webhook n'appelle jamais Telegram** — il authentifie, enfile, acquitte. La production
  d'une réponse appartient au worker.
- **Tout est validé au démarrage** — une faute de déploiement fait échouer le démarrage, pas le
  premier message d'un utilisateur.
- **Un secret est un type, pas une consigne** — `Secret` n'a pas de `Display`, donc
  `format!("{secret}")` ne compile pas. La règle avait déjà échoué deux fois en tant que
  commentaire. Voir [[transport-telegram]].
- **Le coût de chaque appel est inscrit, et ne se réécrit pas** — `consommation` est un registre
  append-only, dès la phase 1.3 et avant tout abonnement : un coût non inscrit au moment de
  l'appel est perdu, et le prix d'un abonnement ne se fixe pas sur une estimation. Voir
  [[client-modele]].
- **On ne suppose pas ce qu'un fournisseur renvoie, on le mesure** — `compagnon modele essai`
  fait l'appel pour de vrai. Deux comportements que des tests simulés auraient manqués sont
  documentés dans [[client-modele]].
