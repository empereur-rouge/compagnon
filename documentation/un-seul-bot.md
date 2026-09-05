---
tags: [reference]
created: 2026-09-05
updated: 2026-09-05
version: v0.2.0
---

# Un seul bot pour toute la plateforme

## La décision

`compagnon` sert **un seul bot Telegram**, partagé par tous ses utilisateurs. Personne n'a
besoin d'en créer un : on ouvre une conversation avec le bot de la plateforme et on discute.
Un `chat_id` identifie une personne, une table `conversation` la relie au personnage qu'elle a
choisi.

C'est le modèle de character.ai, et il est retenu ici parce que **l'utilisateur ne possède
rien** : il vient parler. Lui demander de créer un bot reviendrait à demander à un visiteur
d'héberger son propre serveur.

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

Un seul bot signifie **un seul `@handle` et un seul avatar**. Sophie et Léa apparaissent sous
la même photo dans la liste des discussions de l'utilisateur. C'est le seul vrai prix, et il
est réel : l'identité visuelle d'un personnage est une partie de ce qui le rend présent.

Atténuations, par ordre d'efficacité :

1. **Liens profonds.** `t.me/<bot>?start=sophie` ouvre la conversation directement sur un
   personnage. Telegram transmet le paramètre dans le `/start`, ce qui donne à chaque
   personnage sa propre porte d'entrée sans lui donner son propre bot.
2. **Le portrait en ouverture.** Envoyer la photo du personnage au premier message installe son
   visage dans la conversation, là où l'utilisateur regarde.
3. **Le nom dans le fil.** Le personnage se nomme dans ses messages ; l'icône du bot devient un
   détail de la liste, pas de la conversation.

## Ce que la phase 1 en tire

- **Pas de table `bot`, pas de jeton par client.** Un jeton dans l'environnement, point.
- `utilisateur` est clé sur `chat_id` — l'identifiant Telegram, stable et déjà là.
- `conversation` relie un `utilisateur` à un `personnage` ; c'est elle qui porte la mémoire.
- `/start <parametre>` doit être traité à part dès la phase 1 : c'est à la fois le premier
  contact et le mécanisme de lien profond. En phase 0 il est renvoyé en écho, ce que la carte
  des parcours signale comme la friction n° 2 du produit.

## Interactions

- [[transport-telegram]] — les deux portes d'entrée, et le `chat_id` qui identifie une personne.
- `personnages` (phase 1) — la fiche, sa création, sa modération.
