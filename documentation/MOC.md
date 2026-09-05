---
tags: [moc]
created: 2026-09-05
updated: 2026-09-05
version: v0.1.0
---

# Carte du projet — compagnon

Plateforme de personnages conversationnels sur Telegram. Cette carte est l'index : chaque
fonctionnalité a sa fiche, chaque fiche nomme le code qui la porte.

## Par où commencer

1. [[transport-telegram]] — comment un message entre et comment une réponse sort. Tout le
   reste se branche dessus.
2. [[contrat-d-erreur]] — les codes numériques stables, et pourquoi les messages publics sont
   vagues sur certaines tranches.
3. `README.md` à la racine — démarrer, vérifier, exploiter.

## Fiches

| Fiche | Phase | Sujet |
|---|---|---|
| [[transport-telegram]] | 0 | webhook, authentification, file, envoi, découpage |
| [[contrat-d-erreur]] | 0 | codes numériques, messages publics, journalisation |

## Ce qui viendra

Chaque phase ajoute sa fiche ici. Les intitulés sont fixés d'avance pour que les liens
`[[...]]` écrits en avance pointent un jour quelque part.

| Fiche à venir | Phase | Sujet |
|---|---|---|
| `personnages` | 1 | fiche de personnage, création, modération à la création |
| `moteur-de-dialogue` | 1 | appel du modèle, mise en cache du préfixe, refus |
| `memoire` | 2 | journal roulant, souvenirs structurés, état de relation |
| `medias` | 3 | file de génération à bail, cache de `file_id` |
| `voix` | 4 | synthèse sortante, transcription entrante, transcodage Opus |
| `imagier` | 5 | ancre d'identité, génération à la demande |

## Décisions transverses

- **Le jeton Telegram est dans l'URL** — donc aucune URL n'atteint un journal, une erreur ou un
  `Debug`. Vaut aussi pour le proxy : voir la section correspondante de [[transport-telegram]].
- **Tête-à-tête uniquement** — les messages de groupe sont écartés à l'extraction.
- **Le webhook n'appelle jamais Telegram** — il authentifie, enfile, acquitte. La production
  d'une réponse appartient au worker.
- **Tout est validé au démarrage** — une faute de déploiement fait échouer le démarrage, pas le
  premier message d'un utilisateur.
