---
tags: [reference]
created: 2026-09-05
updated: 2026-09-05
version: v0.2.2
---

# Un seul bot pour toute la plateforme

## La décision

`compagnon` sert **un seul bot Telegram**, partagé par tous ses utilisateurs. Personne n'a
besoin d'en créer un : on ouvre une conversation avec le bot de la plateforme et on discute.
Un `chat_id` identifie une personne, une table `conversation` la relie au personnage qu'elle a
choisi.

L'utilisateur possède bien quelque chose — son assistant, voir [[un-assistant-par-personne]] —
mais pas un bot Telegram. Lui demander d'en créer un reviendrait à demander à un visiteur
d'héberger son propre serveur pour visiter un site.

## Pourquoi cette page existe

La question « comment créer automatiquement un bot pour chaque client ? » se pose
naturellement, et elle n'a pas de bonne réponse. Elle a une bonne **dissolution** : dans ce
modèle, il n'y a qu'un bot, créé une fois, par l'exploitant.

Ce qui suit est là pour qu'on ne repose pas la question dans six mois.

## Ce que Telegram permet, et ce qu'il ne permet pas

**Il n'existe aucune méthode de la Bot API pour créer un bot.** @BotFather n'est pas un
service : c'est un bot avec lequel on converse, et cette conversation passe par MTProto — le
protocole d'un compte *utilisateur*. Vérifié sur `core.telegram.org/bots/api` : la
documentation ne propose aucune alternative.

Deux plafonds s'y ajoutent :

| Contrainte | Valeur |
|---|---|
| Bots par compte Telegram | 20 (40 avec Premium) |
| API pour la photo de profil d'un bot | aucune — @BotFather uniquement |

Ce qui **est** automatisable, une fois qu'on détient un jeton : `setMyName`,
`setMyDescription`, `setMyShortDescription`, `setMyCommands`, `setMyDefaultAdministratorRights`,
`setChatMenuButton`, `setWebhook`.

## Les trois voies, et pourquoi celle-ci

| | Ce que ça demande | Verdict |
|---|---|---|
| **Un bot commun** | rien | **retenu** |
| Un bot par créateur, jeton collé | 3 messages humains une fois, tout le reste par API | tenable si un jour des créateurs veulent leur marque |
| Un bot par client, créé par script | piloter @BotFather via un compte utilisateur MTProto | **écarté** |

La troisième est écartée pour trois raisons, aucune n'étant morale : piloter un compte
utilisateur est proscrit par les CGU de Telegram et vaut des bannissements ; le plafond de 20
impose un compte — donc un numéro de téléphone — par tranche de 20 clients ; et le mécanisme
est du scraping d'une conversation dont Telegram peut changer les formulations sans préavis.
C'est une fondation qui casse au moment précis où le produit marche.

## Ce que cela coûte, et comment on l'atténue

Un seul bot signifie **un seul `@handle` et un seul avatar**. L'assistant de chacun est censé
être le sien, et l'icône dans sa liste de discussions est pourtant celle de la plateforme.
C'est le seul vrai prix, et il est réel : le visage d'un assistant fait partie de ce qui le rend
présent.

Atténuations, par ordre d'efficacité :

1. **Le portrait à la fin de la création.** Envoyer l'image d'ancre au moment où l'utilisateur
   vient de composer son assistant installe ce visage dans la conversation — là où il regarde,
   et au moment où il y est le plus attentif.
2. **Le nom dans le fil.** L'assistant se nomme dans ses messages ; l'icône du bot redevient un
   détail de la liste, pas de la conversation.
3. **Un avatar de plateforme neutre**, qui ne prétend être le visage de personne — plutôt qu'un
   portrait qui contredirait celui de chaque assistant.

Les liens profonds (`?start=…`) ne servent plus à désigner un personnage, puisqu'il n'y a pas de
catalogue. Le paramètre reste libre pour du parrainage.

## Ce que la phase 1 en tire

- **Pas de table `bot`, pas de jeton par client.** Un jeton dans l'environnement, point.
- `utilisateur` est clé sur `chat_id` — l'identifiant Telegram, stable et déjà là.
- `assistant` pend de l'utilisateur, et la mémoire pend de l'assistant : voir
  [[un-assistant-par-personne]].
- `/start` doit être traité à part dès la phase 1 : c'est le parcours de création, donc le
  moment le plus fragile du produit. Il n'est toujours pas traité en phase 1.3 : quelqu'un sans
  compagnon reçoit un message qui le dit, et rien de plus. La carte des parcours le signale
  comme la friction n° 2.

## Interactions

- [[transport-telegram]] — les deux portes d'entrée, et le `chat_id` qui identifie une personne.
- `personnages` (phase 1) — la fiche, sa création, sa modération.
