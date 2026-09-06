---
tags: [feature]
created: 2026-09-06
updated: 2026-09-06
version: v0.10.0
---

# Client modèle — appeler le moteur, et compter ce qu'il coûte

## Résumé

Le module qui fait parler le compagnon. Un trait, [`ClientModele`](#le-trait), pour que le
fournisseur de calcul puisse changer sans que le worker bouge ; une implémentation concrète
contre une API compatible OpenAI ; un double de test qui fabrique les pannes ; et un registre
de coûts en base, rempli dès maintenant pour que la question du prix ait une réponse **mesurée**
quand elle se posera.

Voir aussi [[persistance]] pour la file à bail, et [[transport-telegram]] pour le type `Secret`
et la discipline sur les erreurs.

## Configuration

| Variable | Obligatoire | Rôle |
|---|---|---|
| `MODELE_API_BASE` | oui | racine d'une API compatible OpenAI, `http(s)://…/v1` |
| `MODELE_API_CLE` | oui | clé du fournisseur, portée par un `Secret` |
| `MODELE_NOM` | oui | identifiant du modèle **demandé** |
| `MODELE_FOURNISSEUR` | oui | nom de l'hébergeur, inscrit dans `consommation.fournisseur` |
| `MODELE_JETONS_MAX` | non (500) | jetons de sortie |
| `MODELE_TEMPERATURE` | non (0.85) | échantillonnage |
| `MODELE_DELAI_S` | non (60) | délai de l'appel entier |
| `MODELE_PRIX_ENTREE_EUR_PAR_MILLION` | **oui** | tarif d'entrée |
| `MODELE_PRIX_SORTIE_EUR_PAR_MILLION` | **oui** | tarif de sortie |

Les deux tarifs n'ont **pas** de valeur par défaut. Un défaut à zéro ferait dire au registre que
le service ne coûte rien : la réponse est fausse, elle arrange, et personne ne va la vérifier.

## Modules et fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `modele` | `src/modele/mod.rs` | le trait, `ContexteConversation`, `ReponseModele`, `ErreurModele` |
| `modele::http` | `src/modele/http.rs` | l'appel `POST /chat/completions`, la config, le tarif |
| `modele::double` | `src/modele/double.rs` | un client qui joue un scénario écrit d'avance |
| `panne` | `src/panne.rs` | la nature d'un échec de transport, sans l'URL |
| `db::consommation` | `src/db/consommation.rs` | l'écriture au registre, et la somme d'une période |
| `cli_modele` | `src/cli_modele.rs` | `compagnon modele essai` — un appel réel, et son coût |

## Fonctions clés

| Fonction | Fichier | Description |
|---|---|---|
| `ClientModele::repondre` | `src/modele/mod.rs` | écrit une réponse à partir du contexte |
| `ErreurModele::merite_une_reprise` | `src/modele/mod.rs` | sépare ce qui se rejoue de ce qui se refera échouer |
| `ClientHttp::new` | `src/modele/http.rs` | construit le client, délai compris |
| `ClientModele::cout_eur` | `src/modele/http.rs` | le coût d'un appel au tarif configuré |
| `consommation::inscrire` | `src/db/consommation.rs` | une ligne au registre, et son identifiant |
| `consommation::cout_depuis` | `src/db/consommation.rs` | ce qu'un utilisateur a coûté depuis une date |

## Le trait

`async fn` en trait n'est pas compatible `dyn`, et rendre le worker générique propagerait le
paramètre de type jusqu'à l'état partagé du service et jusqu'au routeur. Les méthodes rendent
donc une `Pin<Box<dyn Future<…> + Send>>` — exactement ce que la caisse `async-trait`
produirait. L'écrire à la main évite une dépendance pour une seule signature.

Sa raison d'être immédiate n'est pas l'interchangeabilité des backends mais l'**éprouvabilité de
la panne** : `modele::double` joue une suite d'actes puis répète le dernier, ce qui rend
« échoue deux fois puis aboutit » exprimable sans variante dédiée.

## Points durs, et ce qui les règle

**Le fournisseur peut mentir par le statut HTTP.** Mesuré sur un vrai serveur compatible OpenAI :
`POST /v9/chat/completions` — un chemin inexistant — rend **`200 OK`** avec
`{"error": "Unexpected endpoint or method."}`. Un faux serveur aurait rendu `404`. Sans le champ
`error`, la réponse se lisait comme une génération vide, donc comme un incident passager, donc
rejouable : une URL fausse épuisait les tentatives en affichant « le modèle n'a rien produit ».
D'où `ErreurModele::RefusApplicatif`, qui ne se rejoue pas.

**Le fournisseur peut répondre avec un autre modèle.** Mesuré également : demander un
`model` inconnu ne produit aucune erreur, le serveur répond avec celui qu'il a chargé et le
nomme dans sa réponse. C'est donc l'identifiant **rendu** qui est inscrit au registre, jamais
celui demandé — sinon la comparaison de coût entre deux versions porte sur ce qu'on croyait
appeler.

**Un modèle à raisonnement peut ne rien écrire.** Il dépense son budget de sortie dans sa
réflexion : mesuré, quatre appels sur cinq à `max_tokens = 80` rendaient `content: ""` avec
`finish_reason: "length"`, et une réponse d'une phrase a coûté 575 jetons de sortie.
`ErreurModele::Tronquee` le distingue de `Vide` — non pour changer la décision (les deux se
rejouent) mais pour le diagnostic : l'un envoie vers `MODELE_JETONS_MAX`, l'autre vers une panne
de modèle.

**Limite connue, phase 1.8.** Certains fournisseurs inscrivent la réflexion dans `content`,
entre balises `<think>`, au lieu d'un champ séparé. Le compagnon enverrait alors sa réflexion à
l'utilisateur. Le filtrage de la sortie appartient aux garde-fous de la phase 1.8 ; il est
signalé pour ne pas être découvert en production.

**Pas de boucle de reprise dans le client.** La file à bail en a déjà une, bornée par
`tentatives_max`, persistante, et qui ne retient aucun worker pendant l'attente. Une seconde
boucle multiplierait les deux — trois tentatives de file × trois d'appel font neuf appels
facturés pour un incident — et retarderait d'autant le moment où l'utilisateur apprend que ça ne
marche pas.

## Le registre des coûts

`consommation` est **append-only**, tenu par un trigger et non par une convention : un coût
modifiable après écriture ne répond plus à la question qu'il sert à répondre. Le trigger compare
la ligne **entière**, pas une liste de colonnes — une liste devrait être tenue à jour à chaque
colonne ajoutée, et une liste oubliée est une garantie qui s'éteint en silence.

Une seule mutation est admise : l'anonymisation exigée par une purge RGPD, qui met
`utilisateur_id`, `conversation_id` et `message_id` à `null` et pose `anonymisee_le`, sans
toucher au montant. C'est pourquoi `utilisateur_id` est nullable là où le schéma d'origine le
voulait `not null` — et pourquoi une contrainte garde les deux moitiés l'une par l'autre :
`(anonymisee_le is null) = (utilisateur_id is not null)`, ce qui interdit d'insérer une ligne
non attribuée.

`cout_fournisseur_eur` est un `numeric(10,6)`, lu en `rust_decimal::Decimal` : les coûts unitaires
sont de l'ordre du millième d'euro, et c'est la somme d'un million de lignes qu'on vient chercher.

## Le chemin d'un message, en phase 1.3

```text
webhook ──► file_messages ──► worker
                                │
                                ├─ âge non vérifié ─────────────► message de service, tâche close
                                ├─ aucun compagnon actif ───────► message de service, tâche close
                                ├─ prompt ≠ son empreinte ──────► message de service, AUCUN appel
                                │
                                ├─ inscrit le message entrant   (avant l'appel : le perdre serait pire)
                                ├─ « en train d'écrire… »
                                ├─ appelle le modèle
                                │     ├─ échec rejouable + tentatives restantes ─► remise en file
                                │     └─ sinon ──► prévient la personne, tâche close
                                ├─ envoie la réponse
                                ├─ inscrit le message sortant
                                └─ inscrit la ligne de coût
```

Trois refus partagent le même épilogue — envoyer, journaliser, clore — dans une seule fonction :
écrit trois fois, il aurait divergé trois fois, et c'est sur ces chemins-là, les moins
parcourus, qu'une divergence ne se voit pas. L'**envoi** de ces messages est repris comme
n'importe quel autre : une coupure réseau ne doit pas faire disparaître le seul message qui
distingue un refus d'une panne.

**Le prompt est vérifié avant chaque appel.** `db::dialogue::ouvrir` recalcule l'empreinte
`sha256` du prompt stocké et refuse d'appeler le modèle si elle ne correspond plus. Le coût est
invisible devant une seconde de génération, et c'est ce qui donne un sens à l'approbation de la
modération : sans lui, un `update` en console suffit à faire parler le compagnon avec un texte
que personne n'a validé. Éprouvé sur le vrai chemin — service réel, base réelle, console `psql` —
le modèle n'est pas appelé et rien n'est facturé.

Cette vérification ne remplace pas `personnage::verifier_integrite`, qui **recompose** depuis les
traits et attrape la dérive éditoriale : c'en est la moitié qu'on peut payer à chaque message.

**Pas de reprise en double.** La file borne déjà les tentatives et les persiste ; le worker s'en
sert avec `merite_une_reprise` pour décider s'il faut les consommer. Une clé invalide échoue en
un appel, un délai dépassé en trois. Quand elles sont épuisées, la personne est prévenue — un
silence serait indiscernable d'un bot mort.

## Ce qui reste à faire, et qui a été mesuré

**Le modèle n'obéit pas toujours aux règles fixes.** Sur le premier essai de bout en bout, à un
message disant « on ne s'est pas parlé depuis des semaines », Alix a répondu « il y a eu un petit
hiatus là » — c'est-à-dire un commentaire sur l'absence, que la **règle fixe 4 interdit
explicitement** et qui figurait dans son prompt. Un prompt n'est pas un mécanisme : les garde-fous
de sortie de la phase 1.8 sont ce qui rendra cette règle tenue plutôt qu'énoncée.

**L'historique n'est pas encore transmis.** `ContexteConversation::echanges` ne porte que le
message courant : la mémoire est la phase 2. Le champ existe déjà pour que son arrivée ne change
aucune signature.

## Interactions

[[persistance]] — la file à bail porte la reprise. [[transport-telegram]] — le type `Secret` et
la classification des pannes. [[compagnon]] — le prompt système que le contexte transporte est
lu en base, jamais recomposé.
