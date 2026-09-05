---
tags: [reference]
created: 2026-09-05
updated: 2026-09-05
version: v0.1.0
---

# Contrat d'erreur

## Résumé

Toute réponse d'erreur du service porte un **code numérique stable** :
`{"code": NNNN, "message": "..."}`. Les clients branchent sur le code ; le message reste libre
d'évoluer. Le contrat vaut pour toutes les réponses — y compris celles produites par le routeur
et les couches `tower`, qui sortiraient nues sans la couche d'enveloppe de `src/http.rs`.

## Grille

| Code | Variante | Statut | Journal | Message public |
|---|---|---|---|---|
| 1001 | `WebhookSecretInvalide` | 401 | WARN | requête non authentifiée |
| 2001 | `PayloadIllisible` | 400 | DEBUG | corps de requête JSON invalide |
| 2002 | `PayloadInattendu` | 400 | DEBUG | le corps ne correspond pas au format attendu |
| 2003 | `ParametreManquant` | 400 | DEBUG | paramètre de requête manquant |
| 2004 | `RouteInconnue` | 404 | DEBUG | route inconnue |
| 2005 | `MethodeNonAutorisee` | 405 | DEBUG | méthode non autorisée pour cette route |
| 2006 | `CorpsTropVolumineux` | 413 | DEBUG | corps de requête trop volumineux |
| 9001 | `Interne` | 503 | ERROR | erreur interne |
| 9002 | `DelaiDepasse` | 503 | ERROR | délai de traitement dépassé |

Tranches réservées pour les phases suivantes : `3xxx` état et règles métier, `4xxx`
cryptographie et sécurité, `5xxx` ressources et quotas.

## Règles

**Ne jamais réattribuer un code.** Un code retiré reste retiré. La numérotation est partagée
avec `agentbot` sur les codes communs (`2004` = route inconnue des deux côtés) : un exploitant
qui tient les deux produits n'a qu'une grille à connaître.

**Les messages de la tranche `1xxx` sont vagues.** Le webhook est une adresse publique.
Distinguer « secret absent » de « secret erroné » en ferait un oracle : on saurait qu'on
approche. Les trois modes d'échec partagent code, statut et message.

**La tranche `2xxx` fait exception.** Décrire un corps JSON illisible ne divulgue rien et fait
gagner du temps en intégration.

**`9001` renvoie `503`, pas `500`.** La défaillance est presque toujours transitoire, et
Telegram rejoue sur `5xx` — ce qui est le comportement voulu : le message d'un utilisateur ne
doit pas disparaître parce que le disque était saturé une seconde.

**Le détail interne ne sort jamais.** `ApiError` porte un détail et sa chaîne de causes ; ils
sont journalisés au moment de la conversion en réponse, et là seulement. Un test
(`aucun_detail_interne_ne_fuit_dans_le_message_public`) construit chaque variante avec un secret
de production dans son détail et vérifie qu'aucun message public ne le laisse passer.

## Modules et Fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `error` | `src/error.rs` | `ErrorCode`, `ApiError`, `CorpsErreur`, `IntoResponse` |
| `http` | `src/http.rs` | `enveloppe_erreur`, `code_pour_statut` |

## Fonctions Clés

| Fonction | Fichier | Description |
|---|---|---|
| `ErrorCode::code` | `src/error.rs` | la valeur numérique — le contrat public |
| `ErrorCode::message_public` | `src/error.rs` | ce qui part sur le fil |
| `ApiError::diagnostic` | `src/error.rs` | le détail et ses causes — journal seulement |
| `http::enveloppe_erreur` | `src/http.rs` | rhabille les erreurs produites hors gestionnaires |

## Interactions

- [[transport-telegram]] — quelle situation renvoie quel code, et pourquoi le partage des
  statuts est dicté par la politique de rejeu de Telegram.
