---
tags: [feature]
created: 2026-09-05
updated: 2026-09-05
version: v0.8.0
---

# Le compagnon — catalogues, traits, prompt, modération

## Résumé

Chaque utilisateur possède **un** compagnon, qu'il compose en choisissant dans des catalogues
fermés. Le service en tire un prompt système, le soumet à la modération, et ce n'est qu'une fois
celle-ci passée que le compagnon devient activable.

Le bot répond toujours en écho — le client de modèle est la phase 1.3. Ce qui existe ici, c'est
tout ce qu'il faudra lui donner.

## Le principe qui gouverne tout

> L'utilisateur choisit des options dans des listes contrôlées. Le service compose le prompt.
> **L'utilisateur ne tape jamais le prompt système lui-même.**

Ce n'est pas une préférence d'architecture, c'est ce qui rend la sûreté **structurelle** : si
aucune valeur du catalogue n'évoque un mineur, aucune composition ne le peut. La modération
porte sur l'ensemble des valeurs possibles, une fois, et non sur chaque compagnon créé.

Trois conséquences pratiques :

- retirer une option, c'est passer un `actif` à faux — pas auditer du texte libre ;
- les données sont exploitables : quels archétypes retiennent, quelle intensité convertit ;
- **le nom est le seul texte libre**, donc le seul endroit qu'il faille réellement examiner.

## Modules et fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `db::catalogues` | `src/db/catalogues.rs` | lecture des vocabulaires contrôlés |
| `personnage` | `src/personnage/mod.rs` | traits, composition, validation, historique |
| `personnage::regles` | `src/personnage/regles.rs` | les quatre règles que rien n'assouplit |
| `personnage::moderation` | `src/personnage/moderation.rs` | examen du nom |
| `cli_compagnon` | `src/cli_compagnon.rs` | création, affichage, vérification d'âge |
| — | `migrations/0003_catalogues.sql` | douze tables de référence, peuplées |
| — | `migrations/0004_compagnon.sql` | les tables `personnage_*`, le verrou, le triangle |
| — | `migrations/0005_moderation.sql` | les termes qu'un nom ne peut pas contenir |

## Fonctions clés

| Fonction | Fichier | Description |
|---|---|---|
| `personnage::charger` | `src/personnage/mod.rs` | lit les traits, résout fusions et plafonds |
| `personnage::composer` | `src/personnage/mod.rs` | **fonction pure** : traits → prompt + empreinte |
| `personnage::valider` | `src/personnage/mod.rs` | compose, modère et inscrit, d'un seul tenant |
| `moderation::examiner_nom` | `src/personnage/moderation.rs` | le seul texte libre à examiner |
| `regles::bloc` | `src/personnage/regles.rs` | les quatre règles, écrites en dernier |

## Ce que la base refuse

Six états impossibles, chacun éprouvé par un test qui constate le refus :

| État | Mécanisme |
|---|---|
| une tranche d'âge sous 25 ans | `check (age_min >= 25)` |
| un compagnon `actif` sans prompt validé | déclencheur `refuser_activation_sans_validation` |
| deux archétypes principaux | index unique partiel |
| un troisième secondaire | `check (rang in (1,2))` + index unique |
| une conversation vers le compagnon d'un autre | clé étrangère **composite** |
| un curseur hors `[0,1]` | `check (valeur between 0.00 and 1.00)` |

Le **verrou d'activation** mérite d'être détaillé. La spécification le décrivait comme
« vérifiable en base par une requête d'audit » — mais vérifiable n'est pas tenu. Une garantie
qu'on constate après coup est une garantie que rien n'empêche d'enfreindre, et celle-ci est la
dernière avant qu'un compagnon ne se mette à parler : elle porte tout ce que la modération aura
décidé. Une contrainte `check` ne peut pas lire une autre table ; un déclencheur, si.

## La composition du prompt

Ordre de résolution, et il n'est pas indifférent — un modèle accorde plus de poids à ce qui
vient en dernier :

1. **identité** — nom et apparence, en libellés résolus depuis les catalogues ;
2. **personnalité** — fusions d'archétypes puis de tons ;
3. **curseurs**, déjà plafonnés par la juridiction ;
4. **registre** — longueur de réponse ;
5. **règles fixes**, toujours en dernier et non paramétrables.

### Les fusions

Un principal, jusqu'à deux secondaires. Si le couple (principal, **premier** secondaire) figure
au catalogue, sa description **remplace** l'addition des deux ; le second secondaire s'ajoute
par-dessus. Une combinaison non répertoriée n'est pas une erreur : les descriptions
s'additionnent simplement.

La fusion est **orientée**. « Principalement timide avec une pointe de dominance » n'est pas
« principalement dominant avec une pointe de timidité » — le yandere est le premier, et une table
qui répondrait dans les deux sens donnerait le mauvais personnage à qui a choisi l'inverse.

### Les curseurs deviennent des paliers

`0,00` à `1,00` en base, cinq paliers nommés dans le prompt : *très peu, peu, modérément,
beaucoup, énormément*.

Écrire « humour : 0,63 » demanderait au modèle d'interpréter une échelle qu'il ne connaît pas, et
deux valeurs voisines produiraient des réponses arbitrairement différentes. Les paliers rendent
la composition **stable**, ce qui a une conséquence concrète : un curseur qui glisse de 0,61 à
0,64 ne change pas le prompt, donc ne redemande pas de modération.

### Les quatre règles fixes

Écrites en dernier, hors de tout catalogue et de tout curseur. Deux interdits — jamais de contenu
impliquant un mineur, jamais de conseil médical ou dangereux — et deux conduites, qui se
distinguent par le **temps** :

| | porte sur | effet recherché |
|---|---|---|
| ravi de parler à son humain | l'instant présent | chaleur, accueil |
| pas de reproche sur une absence | le passé | pas de dette, pas de culpabilité |

« Je suis content que tu sois là » respecte les deux. « Enfin, j'ai cru que tu m'avais oublié »
viole la seconde tout en ayant l'air d'une variante enthousiaste de la première.

## La modération, et ses limites

**Ce qui est structurel** : tout ce qui alimente le prompt vient de catalogues clos. Il n'y a
rien à examiner de ce côté.

**Ce qui ne l'est pas** : le nom. Une partie de son examen l'est quand même — aucun chiffre n'y
est accepté, ce qui élimine d'un coup toute la classe « lea12ans » sans avoir à en énumérer les
graphies. Le reste est un rapprochement de termes, avec ses limites : graphies détournées,
diminutifs, langues absentes de la liste.

**Ce module est la première ligne, pas le classifieur du produit.** Celui-ci arrive avec le
client de modèle en phase 1.3, où le nom pourra être soumis avec son contexte.

Deux détails qui comptent :

- **les termes courts ne sont cherchés que comme mots entiers.** « mere » en sous-chaîne
  refuserait « Meredith », « ado » refuserait « Adolphe ». Un faux refus fait échouer quelqu'un
  qui n'a rien fait, sur son premier geste dans le produit ;
- **le message rendu ne nomme jamais le terme reconnu** — le dire apprendrait quoi contourner.
  Il part au journal d'exploitation.

La liste est une **table** et non une constante : un signalement arrive un dimanche soir, et
attendre une recompilation pour y répondre serait absurde.

## Commandes

```bash
compagnon catalogues                 # tout ce parmi quoi on peut choisir
compagnon compagnon creer utilisateur=42 nom=Léa genre=femme age=25_34 \
                          morphologie=elancee archetype=timide ton=tendre
compagnon compagnon montrer 42       # le prompt composé, avec son empreinte
compagnon utilisateur age 42         # vérification d'âge (support)
```

Les arguments sont des paires `clé=valeur` : sept choix en positionnel se seraient inversés sans
qu'on le voie jusqu'à la lecture du prompt.

`compagnon utilisateur age` existe parce que la phase 1.1 exigeait une vérification d'âge sans
donner aucun moyen de la poser — la seule façon était une écriture SQL directe. Le parcours
d'inscription la remplacera pour l'utilisateur ; celle-ci reste pour le support.

## Ce qui manque encore

- **Le parcours de création dans Telegram.** Tout passe aujourd'hui par la ligne de commande.
- **La modération de l'image d'ancre** (phase 3), qui suivra le même principe : ce que le
  catalogue rend impossible n'a pas à être filtré.
- **La vérification d'âge robuste** : la déclaration simple ne suffit pas dans les juridictions
  qui exigent davantage — et les valeurs par défaut du schéma visent la France.

## Interactions

- [[un-assistant-par-personne]] — pourquoi un seul compagnon par utilisateur.
- [[persistance]] — la file, les workers, et le `chat_id` qui identifie une personne.
- [[contrat-d-erreur]] — les codes numériques stables.
