---
tags: [feature]
created: 2026-09-05
updated: 2026-09-06
version: v0.10.0
---

# Persistance — base, file à bail, consommateurs concurrents

## Résumé

La phase 0 gardait sa file en mémoire : un `kill -9` en perdait le contenu, et le service ne
savait rien d'un utilisateur entre deux messages. Cette phase met PostgreSQL sous le service,
remplace la file par une table **à bail**, et fait consommer **quatre workers en parallèle** au
lieu d'un seul.

Le bot répond toujours en écho. Rien de ce qui suit ne change ce qu'il dit — cette phase change
ce qui survit, et ce qui avance en même temps.

## Ce que ça règle, et pourquoi maintenant

Deux limites, dont une qui n'était pas encore visible.

**La perte au redémarrage** était connue et documentée depuis la phase 0. Une file en mémoire
ne survit pas au processus qui la porte.

**Le sérialisme** ne se voyait pas encore. Traiter un message à la fois donnait l'ordre
gratuitement, et un écho coûte cinquante millisecondes. Dès que la réponse coûtera un appel de
modèle — deux à cinq secondes —, ce même sérialisme fait attendre la centième personne d'une
rafale pendant cinq minutes. Sans erreur, sans journal alarmant : le bot paraît simplement mort.
Le corriger après coup aurait demandé de reprendre la file ; le corriger maintenant ne coûte que
la requête de prise.

## Configuration

| Variable | Rôle |
|---|---|
| `MOTDEPASSE_BASE` | mot de passe PostgreSQL, sert aussi à initialiser le conteneur |
| `DATABASE_URL` | connexion complète ; **secret**, elle porte le mot de passe |

`Config::url_base` est un `Secret` (voir [[transport-telegram]]), dont le `Debug` ne montre
qu'une longueur. Le `Debug` de `Config` en rend davantage, parce qu'un incident commence par
« quelle base ? » — le schéma, l'utilisateur, l'hôte et la base, jamais le mot de passe :
savoir où l'on est connecté est la première question d'un incident, le mot de passe n'a rien à
y faire.

En développement hors Docker : `./scripts/base-de-test.sh demarrer` lance un PostgreSQL jetable
sur le port **5433** — jamais 5432, pour qu'une base installée sur la machine ne soit pas
atteinte par une suite de tests qui crée et détruit des bases.

## Modules et fichiers

| Module | Fichier | Rôle |
|---|---|---|
| `db` | `src/db/mod.rs` | pool, migrations embarquées, sonde |
| `db::file` | `src/db/file.rs` | file à bail : enfiler, prendre, terminer, échouer |
| `db::utilisateurs` | `src/db/utilisateurs.rs` | inscription, vérification d'âge |
| `admission` | `src/admission.rs` | filtre + inscription + mise en file, pour les deux portes |
| `worker` | `src/worker.rs` | les quatre consommateurs |
| `fixtures` | `src/fixtures.rs` | valeurs d'exemple partagées `src/` ↔ `tests/` |
| — | `migrations/0001_noyau.sql` | le schéma |
| — | `migrations/0002_verrou_par_utilisateur.sql` | l'invariant de concurrence, et pourquoi |
| — | `tests/harnais/base.rs` | une base neuve par test, détruite après |

## Fonctions clés

| Fonction | Fichier | Description |
|---|---|---|
| `Base::connecter` | `src/db/mod.rs` | ouvre le pool et **vérifie** qu'une connexion s'établit |
| `file::prendre` | `src/db/file.rs` | prend une tâche, sérialisée par utilisateur |
| `file::echouer` | `src/db/file.rs` | remet en file, ou abandonne au-delà de 3 tentatives |
| `admission::enfiler` | `src/admission.rs` | inscrit puis enfile, dans cet ordre |
| `Base::ouvrir` | `src/db/mod.rs` | joint **et** migre : « un `Base` existe » implique « schéma à jour » |
| `Equipe::lancer` / `eteindre` | `src/worker.rs` | possède les consommateurs, pour que les deux portes cessent de recopier leur montage |
| `config::masquer_url` | `src/config.rs` | retire le mot de passe, en analysant l'URL au lieu de la découper |

## Le schéma, et ce qu'il impose lui-même

Six tables. Ce qui compte n'est pas leur liste mais ce qu'elles rendent **impossible** :

- **un utilisateur → un compagnon → une conversation**, par index uniques partiels. Une règle
  applicative se contourne par une requête oubliée ; sur un produit intime, une fuite de mémoire
  entre deux comptes n'est pas un bug dont on se relève.
- `age_verifie_le` et `methode_verification_age` sont liés par une contrainte : une vérification
  sans méthode serait inauditable, ce qui est le seul usage réel de cette colonne.
- `ref_tranches_age_apparent.age_min >= 25` (phase 1.2) sera de la même nature.
- `erreur_derniere` est un **entier**, jamais un message. C'est la leçon d'un incident de ce
  projet : écrire le `Display` d'une erreur dans une colonne, c'est y écrire un jour un secret
  que l'erreur transportait à l'insu de tous.

## La file à bail

Un état « en cours » nu ne survit pas à la mort du worker qui l'a posé : la tâche reste prise
par personne, et rien ne la reprend jamais. Le bail est une **échéance** — passée celle-ci, la
tâche redevient prenable. Aucun nettoyage périodique n'existe : la requête de prise inclut les
baux expirés dans ses candidats.

```sql
update file_messages f
set statut = 'en_cours', bail_expire_le = now() + …, tentatives = tentatives + 1
where f.id = (
    select c.id from file_messages c
    where (c.statut = 'en_attente' or (c.statut = 'en_cours' and c.bail_expire_le < now()))
      and not exists (…une tâche en vol pour le même utilisateur…)
    order by c.cree_le
    for update skip locked
    limit 1)
returning …
```

## Qui tient quoi

L'invariant à tenir est : **au plus une tâche en vol par utilisateur**. C'est lui qui donne
l'ordre dans une conversation.

| Mécanisme | Rôle |
|---|---|
| `idx_une_tache_en_vol_par_utilisateur` | **tient l'invariant** — index unique partiel |
| `for update skip locked` | deux workers ne prennent jamais la même ligne, aucun n'attend |
| `not exists` | **filtre d'efficacité** : évite la collision, ne la corrige pas |

Une prise concurrente sur le même utilisateur reçoit une violation d'unicité, que le worker
traite comme « rien à prendre » et rejoue.

### Ce que cette table corrige

La première version tenait l'invariant par une composition de `not exists` et d'un
`pg_try_advisory_xact_lock` placé dans le `WHERE`. Elle ne tenait pas, et les trois raisons ont
été mesurées plutôt que supposées :

1. **PostgreSQL donne cette forme en contre-exemple**, textuellement annotée « danger! » : une
   fonction de verrouillage dans un `WHERE` avec `LIMIT` n'est pas garantie d'être évaluée
   après la limite. Mesuré sur ce schéma : **200 verrous posés pour réclamer une seule tâche**,
   tous tenus jusqu'au commit.
2. **La correction dépendait du plan.** Six workers concurrents, mêmes données, même requête :
   plan pipeliné → six servis ; plan avec tri → **un seul**, les cinq autres affamés. Une mise à
   jour de statistiques suffisait à basculer, sans erreur ni journal.
3. **La course restait ouverte.** En `read committed`, deux workers dont les instantanés
   précèdent le commit de l'autre voient tous deux le `not exists` vrai. Reproduit : deux tâches
   du même utilisateur prises simultanément — donc l'ordre que le service promet ne tenait que
   par le fait que le plan choisi était le bon.

Un index unique partiel vérifie l'unicité à l'insertion de l'entrée d'index, sous verrou de
page, hors MVCC : c'est le seul mécanisme qui tienne quel que soit le plan, le niveau
d'isolation et l'ordre d'évaluation des clauses. C'est aussi celui que ce schéma emploie déjà
pour la cardinalité — la file était le seul endroit où la concurrence mord réellement, et le
seul parti sur une règle applicative.

## Concurrence : entre les conversations, pas dans une conversation

Quatre workers. Le worker lui-même n'a **aucune** synchronisation : c'est l'index unique
partiel qui la lui donne. Vingt messages d'une même personne ressortent dans l'ordre pendant que les
autres conversations avancent en parallèle — éprouvé par
`l_ordre_est_tenu_dans_une_conversation_malgre_les_workers_concurrents`.

Les workers **scrutent** la file (250 ms au repos) plutôt que d'être réveillés par un
`LISTEN/NOTIFY`. Compromis assumé de cette phase : la latence ajoutée est bornée, et l'absence
de canal de notification retire une pièce mobile au moment où la file elle-même est neuve. À
reprendre quand la latence comptera plus que la simplicité, c'est-à-dire quand la réponse ne
sera plus un écho.

## Ce que l'extinction garantit désormais

Elle a changé, et dans le bon sens.

| | Phase 0 | Phase 1.1 |
|---|---|---|
| Promesse | vider entièrement la file | finir les tâches **en cours** |
| Ce qui reste | rien — ou perdu au `SIGKILL` | reste en base, repris au démarrage |
| Risque en cas de dépassement | messages perdus en silence | aucun |

Attendre la fin des tâches en cours n'est pas du zèle : une tâche interrompue serait reprise au
bail, et l'utilisateur recevrait deux fois la même réponse.

## Bornes

| Borne | Valeur | Pourquoi |
|---|---|---|
| Tâches en file par utilisateur | 32 | une table n'est pas bornée par construction |
| Tentatives avant abandon | 3 | |
| Bail | 120 s | doit couvrir le cas le plus lent, pas le plus fréquent |
| Repos après échec | 25 ms | sans lui, quatre workers épuisent les trois tentatives en quelques millisecondes, et une panne passagère de Telegram fait abandonner un message qui serait passé au coup suivant |
| Connexions du pool | 16 | plus large que 4 workers, pour que la sonde ne soit pas affamée |

La borne de file est **par utilisateur** et non globale : une borne globale se retourne contre
les mauvaises personnes, un seul émetteur en rafale la remplissant et faisant refuser tous les
autres. Le refus rend `503` (code `5001`), donc Telegram rejoue — le message est différé, pas
perdu.

## Vérification d'âge

`age_verifie_le` nul ⇒ pas d'accès au moteur, dès cette phase, même en écho. Le refus produit un
**message** qui dit ce qui manque : un silence serait indiscernable d'une panne, et c'est la
première friction que la carte des parcours signale.

Le contrôle est fait dans le worker et non à l'entrée, parce que c'est le worker qui parle à
l'utilisateur. En phase 1.3, c'est au même endroit qu'il empêchera l'appel au modèle.

## Tests

`tests/persistance.rs` éprouve les quatre promesses plutôt que de les annoncer :

| Test | Ce qu'il prouve |
|---|---|
| `une_tache_non_traitee_survit_a_l_arret_du_service` | un second service reprend ce que le premier a laissé |
| `un_bail_expire_est_repris_par_un_autre_worker` | la mort d'un worker ne perd rien |
| `l_ordre_est_tenu_dans_une_conversation_malgre_les_workers_concurrents` | 20 messages, 4 workers, ordre intact |
| `la_file_est_bornee_par_utilisateur` | 32 acceptés, refus en `503` |
| `sans_verification_d_age_le_moteur_reste_ferme` | le refus nomme ce qui manque, l'écho ne part pas — et il part une fois l'âge vérifié |
| `aucune_forme_d_url_de_base_ne_laisse_fuir_le_mot_de_passe` | cinq formes que `sqlx` accepte, toutes muettes |

Chaque test reçoit une **base PostgreSQL neuve**, migrée par le code de production puis
détruite. Ni transaction annulée — le service ouvre son propre pool et ne verrait rien de ce que
la transaction du test aurait écrit —, ni schéma partagé — les migrations se poseraient au
mauvais endroit. C'est aussi la seule forme qui permet d'éprouver la concurrence, c'est-à-dire
précisément ce que cette phase ajoute.

## Interactions

- [[transport-telegram]] — les deux portes d'entrée, qui enfilent toutes deux par `admission`.
- [[contrat-d-erreur]] — `5001` file saturée, `9003` envoi impossible, `9004` tâche illisible.
- [[un-assistant-par-personne]] — la cardinalité que le schéma impose.
- [[un-seul-bot]] — pourquoi `utilisateurs.id` est l'identifiant Telegram.
