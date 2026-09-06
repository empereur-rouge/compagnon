# Changelog

Toutes les modifications notables de ce projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/), et le projet applique
le [versionnage sémantique](https://semver.org/lang/fr/).

## [0.10.0] - Unreleased

Phase 1.3a — le contrat du moteur de dialogue, avant tout appel réel.

### Added

- **feat(secret)** : `Secret`, une valeur qui ne **peut pas** atterrir dans un journal. Pas de
  `Display` — `format!("{secret}")` ne compile pas — pas de `Deref<Target = str>`, et un `Debug`
  qui rend `<masqué, N caractères>`. Ce projet a laissé fuir un secret deux fois, et les deux
  fois la règle existait dans un commentaire plutôt que dans un type. `exposer()` est nommé pour
  être désagréable : `rg 'exposer\('` donne la liste exhaustive des points de sortie.
- **feat(modele)** : le trait `ClientModele`, avec `ContexteConversation`, `ReponseModele` et
  `ErreurModele`. Le fournisseur de calcul va changer — serverless d'abord, GPU dédié ensuite —
  et le worker ne doit pas bouger pour autant.
- **feat(modele)** : `ErreurModele::merite_une_reprise()` distingue ce qui se rejoue (délai,
  connexion, génération vide, `429`, `5xx`) de ce qui se refera échouer à l'identique (`400`,
  `401`, `403`). Sans cette distinction, une clé invalide consomme toutes les tentatives et un
  délai dépassé perd le message de quelqu'un qui l'attend.
- **feat(modele)** : `modele::double` — un `ClientModele` qui joue un scénario écrit d'avance
  puis en répète le dernier acte. « Échoue deux fois puis aboutit » s'écrit donc sans variante
  dédiée, et les pannes du fournisseur — qui sont rares, non reproductibles, et arrivent en
  production — deviennent éprouvables.
- **test** : six tests sur le contrat, dont `le_prompt_arrive_au_modele_tel_quel`, qui fixe la
  moitié aval d'une garantie décidée en 1.2 : le worker lira `prompt_systeme_genere` plutôt que
  de recomposer les traits, parce que c'est le texte que la modération a approuvé.

- **feat(modele)** : `modele::http::ClientHttp`, l'implémentation concrète contre une API
  compatible `POST /chat/completions`. Retenue non par attachement à un fournisseur mais parce
  que vLLM, TGI et la quasi-totalité des hébergeurs de GPU l'exposent déjà : c'est ce qui rend
  le remplacement de backend réel plutôt que théorique.
- **feat(db)** : migration `0007_consommation.sql` et `db::consommation`. Registre **append-only**
  tenu par trigger : ni `update` ni `delete`, à une exception décrite exactement — l'anonymisation
  RGPD, qui détache la ligne de son utilisateur sans en perdre le montant. Le trigger compare la
  ligne entière plutôt qu'une liste de colonnes, pour que toute colonne future soit couverte le
  jour où elle apparaît.
- **feat(cli)** : `compagnon modele essai <texte>` — un appel réel au fournisseur configuré, qui
  imprime la réponse, le modèle **rendu**, les jetons, la durée mesurée et le coût à six
  décimales. C'est l'outil qui a trouvé les deux défauts ci-dessous.
- **test** : neuf tests du client HTTP rejouant des formes de réponse **relevées sur un vrai
  serveur**, et cinq tests du registre sur un vrai PostgreSQL — dont
  `tout_le_vocabulaire_rust_est_accepte_par_la_base`, qui écrit les 45 combinaisons de
  `type` × `origine` × `statut` et attrape une variante ajoutée d'un seul côté.

### Fixed

- **fix(modele)** : un fournisseur qui répond **`200 OK`** avec `{"error": …}` était lu comme une
  génération vide, donc comme un incident passager, donc rejoué. Constaté sur un vrai serveur :
  c'est ce qu'il rend sur un chemin inexistant, là où un faux serveur aurait rendu `404`. Une URL
  mal saisie épuisait donc les tentatives en affichant « le modèle n'a rien produit ».
  `ErreurModele::RefusApplicatif` la classe désormais comme permanente.
- **fix(modele)** : une génération coupée par la limite de jetons **avant tout texte** rendait le
  même message qu'un modèle réellement muet. Mesuré sur un modèle à raisonnement : quatre appels
  sur cinq à `max_tokens = 80` rendent `content: ""` avec `finish_reason: "length"`.
  `ErreurModele::Tronquee` envoie vers `MODELE_JETONS_MAX` au lieu d'envoyer chercher une panne
  de modèle.

### Changed

- **change(panne)** : `Panne` quitte `telegram::envoi` pour `crate::panne`. Deux copies du même
  énuméré avaient coexisté le temps d'un commit, et deux copies d'une garantie divergent — celle
  qu'on oublie de corriger devient celle par laquelle la fuite revient.
- **change(secret)** : les trois secrets existants passent par `Secret` — `Config::jeton_bot`,
  `Config::secret_webhook`, `Config::url_base`, ainsi que `Canal::racine` et `Canal::secret`.
  Leur protection reposait jusqu'ici sur un `Debug` écrit à la main et sur l'**absence** de
  dérivations : deux garanties qui ne couvrent que ce qu'elles nomment, alors que le champ
  restait une `String` que n'importe quel `format!` un cran plus bas pouvait rendre.
- **change(telegram)** : `Canal` dérive `Debug`, après l'avoir longtemps refusé. Ses deux champs
  secrets étant des `Secret`, la dérivation **est** le rendu masqué — et un rendu masqué vaut
  mieux qu'une interdiction, qui laissait `format!("{:?}", canal.racine)` passer sans un mot du
  compilateur. `tests/secrets.rs` imprime désormais ce rendu.
- **change(modele)** : `ErreurModele` classe la panne au lieu de la transporter — `Panne` est un
  enum nu, `Refuse` ne porte qu'un `u16`, et le corps d'une réponse de fournisseur n'entre nulle
  part. Même correctif que `telegram::envoi::Panne`, appliqué **avant** la fuite plutôt qu'après :
  un message d'erreur de fournisseur reprend souvent la requête, donc le prompt système, donc
  tout ce que le compagnon est.

## [0.9.1] - 2026-09-05

### Changed

- **change(db)** : les écritures d'un compagnon quittent le module de ligne de commande pour
  `db::personnages`. Elles y étaient privées, avec deux conséquences dont la seconde s'était
  déjà produite : le parcours d'inscription de la phase 1.3 n'aurait pu en réutiliser aucune, et
  **les tests les avaient déjà recopiées** — en omettant le `and actif` que la production
  applique. Ils construisaient donc des compagnons sur des lignes de catalogue désactivées, et
  aucun test ne pouvait attraper une régression sur ce filtre, qui est pourtant le mécanisme de
  retrait dont la migration 0003 fait un argument de sûreté.

### Added

- **test** : `une_option_retiree_du_catalogue_est_refusee_a_l_ecriture` — le test qui était
  **impossible** à écrire tant que les écritures étaient privées. Le mécanisme de retrait
  rétroactif n'était éprouvé nulle part.
- **test** : les fabriques de compagnon passent désormais par le chemin de production, comme le
  harnais l'avait fait pour `verifier_age` en phase 1.1.

## [0.9.0] - 2026-09-05

Revue `/simplify` de la phase 1.2. Quatre agents, une cinquantaine de findings. Ce qui suit
corrige un seul motif, répété cinq fois : **la garantie s'arrêtait au moment précis où elle
aurait dû porter sur du texte plutôt que sur une forme.**

### Fixed

- **fix(db)** : **la thèse centrale de la phase 1.2 était fausse.** « Si aucune valeur du
  catalogue n'évoque un mineur, aucune composition ne le peut » reposait sur une prémisse jamais
  écrite — les tables `ref_*` sont immuables en production — que rien ne rendait vraie.
  Reproduit : `update ref_tranches_age_apparent set libelle = 'Adolescente de 16 ans'` passait la
  contrainte `age_min >= 25`, passait les tests, passait la modération, et le prompt disait
  « Femme, Adolescente de 16 ans ». Le texte éditorial du catalogue est désormais immuable hors
  migration ; `actif` reste modifiable, pour que le retrait à chaud garde sa force.
- **fix(personnage)** : l'âge est composé depuis `age_min`, le **nombre contraint**, et non
  depuis le libellé de la tranche. La contrainte gardait une colonne que la composition ne
  lisait pas.
- **fix(db)** : **le mécanisme de plafond légal était branché sur rien.** La jointure filtrait
  `domaine = 'personnalite'` alors que le seul paramètre marqué `plafonnable_juridiction` est
  `intensite_suggestive`, de domaine `contenu` : les plafonds ne pouvaient s'appliquer qu'à des
  paramètres déclarés *non* plafonnables. Une colonne `entre_dans_le_prompt` nomme désormais ce
  que le domaine servait à déduire.
- **fix(db)** : **le verrou d'activation ne gardait que l'instant de la transition.** Après
  validation, les traits et le nom restaient librement modifiables — un compagnon pouvait rester
  actif en portant un prompt qui ne le décrivait plus, et un nom jamais modéré. C'était le second
  chemin par lequel du texte non modéré atteignait le modèle. Toute modification révoque
  désormais la validation et rabat le statut.
- **fix(db)** : `intensite_suggestive` avait **deux domiciles** — la création en écrivait une
  copie sur le compagnon alors que la spécification le donne à l'utilisateur. Deux sources de
  vérité pour le seul paramètre à conséquence légale. Rendu inexprimable.
- **fix(cli)** : la création n'était **pas transactionnelle**. Un échec au milieu laissait la
  ligne `personnages` committée, et l'index unique interdisait alors toute nouvelle tentative
  pour cet utilisateur.
- **fix(cli)** : les messages d'erreur annonçaient toute défaillance — connexion perdue comprise
  — comme une faute de frappe, et interpolaient le `Display` de `sqlx` : ce que la migration 0001
  interdit explicitement, « c'est exactement le chemin par lequel un jeton fuirait ».
- **fix(test)** : le test du plafond **prouvait le défaut en le prenant pour le comportement
  attendu** — il posait le plafond sur un paramètre non plafonnable et constatait qu'il
  s'appliquait.

### Added

- **feat(personnage)** : `activer`, **seul écrivain** de `statut = 'actif'`. Il n'en existait
  aucun : la validation laissait en `brouillon`, la CLI annonçait « activable », et le seul
  chemin vers l'état actif était un `psql`. Le verrou construit pour protéger ce geste gardait
  une porte que le produit ne savait pas ouvrir.
- **feat(personnage)** : `verifier_integrite`. `prompt_systeme_hash` était écrit et jamais relu —
  et comparé à lui seul, il n'aurait rien attrapé d'utile, vivant dans la même ligne que le texte
  qu'il atteste. La comparaison qui a de la valeur est la seconde : recomposer depuis les traits
  actuels. Elle détecte ce qu'aucune contrainte ne peut voir.
- **feat(cli)** : `compagnon compagnon activer` et `compagnon compagnon verifier`.
- **feat(personnage)** : la création inscrit enfin une version `'creation'` à l'historique — la
  valeur figurait dans la contrainte et n'était jamais produite.

### Removed

- Deux index qui documentaient une intention que le code n'avait pas : zéro balayage mesuré sur
  les deux, et l'un doublé par l'index de la contrainte unique.
- Une ligne morte de `ref_termes_interdits` : accentuée, donc jamais rapprochée d'un nom
  normalisé sans accents.

## [0.8.0] - 2026-09-05

Phase 1.2e — la création, et le geste d'exploitation qui manquait.

### Added

- **feat(cli)** : `compagnon catalogues`, `compagnon compagnon creer`,
  `compagnon compagnon montrer`, `compagnon utilisateur age`. Les arguments sont des paires
  `clé=valeur` : sept choix en positionnel se seraient inversés sans qu'on le voie jusqu'à la
  lecture du prompt. Le dépôt ne gagne pas d'analyseur d'arguments pour autant.
- **feat(cli)** : `compagnon utilisateur age` existe parce que la phase 1.1 exigeait une
  vérification d'âge **sans donner aucun moyen de la poser** — la seule façon était une écriture
  SQL directe, ce qu'il a fallu faire à la main lors de l'essai de bout en bout. Le parcours
  d'inscription la remplacera pour l'utilisateur ; celle-ci reste pour le support.

### Changed

- **change(personnage)** : le type `Cible` — quelle composition, archétypes ou tons — devient
  public et sert aux deux modules. Les trois noms de table voyagent toujours ensemble ; passés
  séparément ils faisaient huit arguments, et rien n'empêchait de mélanger la table de liaison
  des archétypes avec la référence des tons.

### Notes

- La création ne pose que des **choix**, puis appelle la validation. Il n'existe aucun chemin,
  dans la CLI ni ailleurs, par lequel un texte saisi atteindrait le modèle.
- Éprouvé sur le vrai chemin : un compagnon créé sur la base de production de développement,
  avec la fusion Yandere résolue depuis le catalogue, et un nom refusé qui n'a laissé aucun
  prompt derrière lui.

## [0.7.0] - 2026-09-05

Phase 1.2d — la modération, et ce qu'elle protège réellement.

### Added

- **feat(moderation)** : examen du **nom**, seul texte libre d'un compagnon. Tout le reste vient
  de catalogues clos : une apparence ne peut pas évoquer un mineur parce que la base refuse
  toute tranche sous 25 ans, une personnalité ne peut pas dériver parce que les descriptions
  sont écrites au catalogue. Le nom est l'unique interstice, et c'est le seul endroit à examiner.
- **feat(moderation)** : `ref_termes_interdits`, une **table** et non une constante — un
  signalement arrive un dimanche soir, et attendre une recompilation pour y répondre serait
  absurde. L'inverse vaut aussi : un terme trop large se retire sans déploiement, ce qu'un test
  éprouve.
- **feat(personnage)** : `valider` compose, examine et inscrit **d'un seul tenant**. Séparer ces
  gestes laisserait exister un instant où un prompt est écrit sans que la modération se soit
  prononcée — précisément l'état que le verrou d'activation existe pour empêcher.
- **feat(personnage)** : l'historique versionné, avec un instantané complet construit par la
  base en une requête. Un refus y est inscrit comme une validation : il fait partie de ce qu'on
  doit pouvoir raconter.

### Notes

- **Ce qui est structurel et ce qui ne l'est pas.** Les chiffres sont refusés dans un nom sans
  exception, ce qui élimine d'un coup toute la classe « lea12ans » sans avoir à en énumérer les
  graphies. Le rapprochement de termes, lui, est heuristique et le reste : il rate les graphies
  détournées, les diminutifs, les langues absentes de la liste. **Ce module est la première
  ligne, pas le classifieur du produit** — celui-ci arrive avec le client de modèle en 1.3.
- **Les termes courts ne sont cherchés que comme mots entiers.** « mere » en sous-chaîne
  refuserait « Meredith », « ado » refuserait « Adolphe ». Un faux refus n'est pas gratuit : il
  fait échouer quelqu'un qui n'a rien fait, sur son premier geste dans le produit. Seize noms
  ordinaires sont éprouvés comme passant, dont ceux-là.
- **Le message rendu ne nomme jamais le terme reconnu** — le dire apprendrait quoi contourner.
  Il part au journal d'exploitation.

## [0.6.0] - 2026-09-05

Phase 1.2c — la composition du prompt.

### Added

- **feat(personnage)** : `composer` produit le prompt système à partir des traits, dans l'ordre
  du document — identité, personnalité, curseurs plafonnés, registre, puis **règles fixes en
  dernier**. L'ordre n'est pas cosmétique : un modèle accorde plus de poids à ce qui vient en
  dernier, et aucune valeur de paramètre ne doit pouvoir contredire ces règles.
- **feat(personnage)** : `regles`, les quatre règles que rien n'assouplit. Les deux premières
  sont des interdits, les deux suivantes décrivent une conduite — et la distinction entre elles
  est celle du **temps** : « ravi de parler à son humain » porte sur l'instant présent, « pas de
  reproche sur une absence » sur le passé. « Je suis content que tu sois là » respecte les deux ;
  « j'ai cru que tu m'avais oublié » viole la seconde en ayant l'air d'une variante de la
  première.
- **feat(personnage)** : résolution des fusions telle que le document la définit — la fusion
  **remplace** l'addition des deux descriptions, consomme le principal et le premier secondaire,
  et le second s'ajoute par-dessus.
- **feat(personnage)** : les plafonds de juridiction sont appliqués **dans la requête**, pas
  après coup — une vérification qu'une évolution du code applicatif ne peut pas contourner.
- **feat(personnage)** : empreinte SHA-256 du prompt, pour détecter un écart introduit hors du
  processus.

### Notes

- **Les curseurs deviennent des paliers, pas des nombres.** « Humour : 0,63 » demande au modèle
  d'interpréter une échelle qu'il ne connaît pas, et deux valeurs voisines produiraient des
  réponses arbitrairement différentes. Cinq paliers nommés rendent la composition stable : un
  curseur qui glisse de 0,61 à 0,64 ne change pas le prompt, donc ne redemande pas de modération.
- **`composer` est une fonction pure**, séparée de la lecture en base. C'est ce qui permet de
  **lire le prompt produit** dans la sortie des tests — c'est le texte qui compte, pas le fait
  que la fonction rende `Ok`. Deux défauts n'ont été trouvés que par cette lecture : une espace
  parasite avant une virgule, et le code interne `mi_longs` livré tel quel au modèle.

## [0.5.0] - 2026-09-05

Phase 1.2b — les tables du compagnon, et ce que la base refuse d'en faire.

### Added

- **feat(db)** : `personnage_apparence`, `personnage_archetypes`, `personnage_tons`,
  `personnage_parametres_gradues`, `personnage_parametres_interaction`,
  `personnage_parametres_modele`, `personnage_historique_versions`.
- **feat(db)** : **le verrou d'activation**. La spécification disait « `valide_le` nul ⇒ le
  compagnon ne peut pas passer en `actif`, vérifiable en base par une requête d'audit ».
  Vérifiable n'est pas tenu : un déclencheur refuse désormais l'activation, quel que soit le
  chemin d'écriture. C'est la dernière garantie avant qu'un compagnon ne se mette à parler, et
  elle porte tout ce que la modération aura décidé.
- **feat(db)** : un principal obligatoire et au plus deux secondaires, par index uniques
  partiels ; plus une contrainte croisée `rôle`/`rang` — un principal rangé ou un secondaire
  sans rang décriraient un état que la résolution du prompt ne saurait pas lire.

### Fixed

- **fix(db)** : **le triangle utilisateur / compagnon / conversation était ouvert.** Trois index
  uniques garantissaient trois bornes indépendantes, mais les deux clés étrangères de
  `conversations` étaient disjointes : une écriture directe pouvait relier l'utilisateur A au
  compagnon de B. Sur un produit intime, c'est le chemin par lequel la mémoire de quelqu'un
  atterrit chez un autre. Une clé étrangère composite rend la construction impossible au lieu de
  la garder en trois endroits.

## [0.4.0] - 2026-09-05

Phase 1.2 — les catalogues. Première des cinq tranches du schéma compagnon.

### Added

- **feat(db)** : les vocabulaires contrôlés dans lesquels l'utilisateur choisit — genres,
  morphologies, couleurs de cheveux et d'yeux, styles vestimentaires, tranches d'âge apparentes,
  vingt archétypes, treize tons, leurs fusions nommées orientées, et six curseurs gradués avec
  leurs plafonds par juridiction.
- **feat(db)** : `db::catalogues`, la lecture de ces tables. Les cinq catalogues d'apparence
  partagent une seule fonction, le nom de table venant d'un énuméré fermé — sûr précisément
  parce qu'aucune chaîne extérieure ne peut l'atteindre.
- **feat(db)** : `rust_decimal` pour les curseurs. Les décoder en `f64` réintroduirait dans le
  code l'imprécision que `numeric(3,2)` refuse en base.

### Notes

- **Le peuplement vit dans la migration**, pas dans un script à part : ces valeurs ne sont pas
  des données d'exemple mais des constantes du produit. Une base migrée sans elles laisserait le
  service incapable de créer un compagnon — panne qui ne se déclarerait qu'au premier
  utilisateur.
- **`age_min >= 25` est une contrainte de base, pas une convention.** Le plancher est à 25 et
  non à 18 : une apparence proche de la limite est exactement la zone qu'aucun classifieur ne
  tranche de façon fiable, et que ce produit n'a aucune raison d'explorer. Éprouvé par des
  insertions à 16, 18 et 24 ans, toutes refusées.
- **La sûreté est structurelle** : si aucune valeur du catalogue n'évoque un mineur, aucune
  composition ne le peut. La modération porte sur l'ensemble des valeurs possibles, une fois,
  et non sur chaque compagnon créé.

## [0.3.0] - 2026-09-05

Phase 1.1 — la persistance. Livrée en deux temps : la couche base, puis la bascule.

### Added

- **feat(db)** : PostgreSQL, avec `sqlx` et des migrations versionnées embarquées dans le
  binaire. Le conteneur livré ne contient pas l'arbre source ; lire les migrations sur disque
  aurait produit un service tournant sur un schéma incomplet plutôt qu'un refus franc.
- **feat(db)** : schéma du noyau — `utilisateurs`, `personnages`, `conversations`, `messages`,
  `historique_consentement`, `file_messages`. La cardinalité **un utilisateur → un compagnon →
  une conversation** est tenue par des index uniques partiels, pas par une règle applicative.
- **feat(db)** : file de traitement **à bail**. Un état « en cours » nu ne survit pas à la mort
  du worker qui l'a posé : la tâche reste prise par personne et rien ne la reprend. Le bail est
  une échéance, et la requête de prise inclut les baux expirés dans ses candidats — aucun
  nettoyage périodique n'est nécessaire.
- **feat(config)** : `DATABASE_URL`, traitée comme un secret. Le `Debug` ne montre que schéma,
  utilisateur, hôte et base — savoir où l'on est connecté est la première question d'un
  incident, le mot de passe n'a rien à y faire.
- **test** : `tests/harnais/base.rs` — une base PostgreSQL **neuve par test**, migrée par le
  code de production puis détruite. Ni transaction annulée (le service ouvre son propre pool et
  ne verrait rien) ni schéma partagé (les migrations se poseraient au mauvais endroit).
- **outil** : `scripts/base-de-test.sh` démarre le PostgreSQL de test sur le port 5433 — jamais
  5432, pour qu'une base de développement installée sur la machine ne soit pas atteinte par une
  suite qui crée et détruit des bases.

- **feat(worker)** : **quatre consommateurs concurrents** au lieu d'un seul. L'ordre reste tenu
  là où il compte — dans une conversation — par la requête de prise, qui écarte tout
  utilisateur déjà servi ailleurs. Le worker n'a donc aucune synchronisation à faire : la base
  la lui donne. Sans cela, cent personnes écrivant dans la même minute feraient attendre la
  centième cinq minutes dès que la réponse coûtera un appel de modèle, sans qu'aucune erreur ne
  soit journalisée.
- **feat(worker)** : la vérification d'âge barre l'accès au moteur, dès cette phase. Un refus
  produit un message qui dit ce qui manque, jamais un silence — un silence est indiscernable
  d'une panne.
- **feat(db)** : la file est **bornée par utilisateur** (32 tâches). Une table n'est pas bornée
  par construction, contrairement au canal de la phase 0, et une borne globale se retourne
  contre les mauvaises personnes : un seul émetteur en rafale la remplirait et ferait refuser
  tous les autres.
- **feat(deploiement)** : service `base` dans `compose.yaml`, sans port publié, avec sonde
  `pg_isready` dont le service attend le vert — le service migre au démarrage et doit trouver
  une base qui répond, pas seulement un conteneur lancé.

### Changed

- **change(app)** : **ce que l'extinction garantit a changé, et dans le bon sens.** Elle ne
  vide plus la file : ce qu'elle contient survit à l'arrêt et sera repris au démarrage suivant.
  Elle attend seulement la fin des tâches en cours, pour qu'aucune ne soit reprise au bail et
  répondue deux fois.
- **change(http)** : la sonde rend `base_repond` et `taches_en_attente` au lieu des places
  libres du canal. Une base qui répond avec une file qui enfle est un cas bien plus fréquent
  qu'une base muette, et les confondre en un seul booléen le rendrait indétectable.
- **change(admission)** : l'inscription de l'utilisateur et la mise en file sont un seul geste
  partagé par les deux portes d'entrée, dans cet ordre — la clé étrangère l'impose.
- **change(test)** : les valeurs d'exemple et la construction de `Config` vivent dans
  `compagnon::fixtures`, derrière la caractéristique `fixtures`. Elles étaient recopiées dans
  six fichiers sur deux cibles de compilation — une revue l'avait signalé, et l'ajout d'un seul
  champ à `Config` a cassé les six le jour même.

## [0.2.2] - 2026-09-05

### Infrastructure

- **docs** : consigne le modèle produit — **un assistant par personne, qui lui appartient**, et
  non un catalogue de personnages partagés. C'est le modèle Replika et non celui de
  character.ai, et la différence décide du schéma, du coût, de la modération et de l'onboarding.
  Quatre conséquences documentées : le cache d'invite n'est plus mutualisé entre utilisateurs ;
  la modération porte sur toute la base et non sur quelques créateurs ; `/start` devient le
  parcours de création, donc le moment le plus fragile du produit ; l'ancre d'identité doit être
  générée par assistant, ce qui exclut une LoRA par personne.
- **docs** : corrige `un-seul-bot.md`, écrit sur l'hypothèse d'un catalogue partagé. Les liens
  profonds ne désignent plus un personnage — le paramètre `?start=` reste libre pour du
  parrainage — et le décalage d'avatar est plus sensible qu'annoncé, l'assistant étant censé
  être celui de l'utilisateur.

## [0.2.1] - 2026-09-05

### Infrastructure

- **docs** : consigne la décision « un seul bot pour toute la plateforme », avec ce que
  Telegram permet et interdit — aucune API de création de bot, plafond de 20 bots par compte,
  aucune API pour l'avatar d'un bot. La question « comment créer un bot par client » n'a pas de
  bonne réponse mais une bonne dissolution, et cette page existe pour qu'on ne la repose pas.

## [0.2.0] - 2026-09-05

Recevoir sans rien déployer.

### Added

- **feat(scrutation)** : `compagnon ecouter` reçoit par `getUpdates` au lieu d'attendre un
  appel entrant. Ni domaine, ni certificat, ni tunnel, ni compte tiers : une connexion sortante
  suffit, donc n'importe quel poste de travail. Sans cela, le premier essai d'un développeur
  contre un vrai compte Telegram imposait un tunnel et une dépendance externe.
- **feat(telegram)** : `Canal::recevoir_mises_a_jour`, avec un délai propre à cet appel — une
  scrutation longue tient la connexion plusieurs dizaines de secondes, très au-delà du délai
  qui convient à tout le reste. Sans cette dérogation, le client trancherait sa propre attente
  et la scrutation dégénérerait en sondage serré.

### Changed

- **change(admission)** : le filtrage d'une mise à jour et sa journalisation quittent
  `webhook.rs` pour un module `admission`, appelé par les deux portes d'entrée. Un mode de
  développement qui emprunterait un chemin parallèle ne dirait rien du comportement en
  production ; l'identité des deux chemins est désormais structurelle, et testée.
- **change(scrutation)** : le webhook est retiré au démarrage de `ecouter`, et le retrait est
  journalisé. Telegram interdit de mêler les deux modes et répond `409` sinon — un `409` est
  d'ailleurs traduit en message nommant les deux causes réparables.

### Notes

- La production reste au webhook : la scrutation tient une connexion ouverte en permanence, ne
  se répartit pas sur plusieurs instances, et redemande à Telegram au lieu d'être servie.
- **Scruter avec le jeton de production coupe la production**, puisque le webhook est retiré.
  C'est journalisé, pas silencieux.

## [0.1.0] - 2026-09-05

Phase 0 — la boucle de transport, prouvée de bout en bout.

### Added

- **transport** : webhook Telegram sur Axum, authentifié par le secret partagé
  `X-Telegram-Bot-Api-Secret-Token`, comparé en temps constant. Les trois modes d'échec
  (en-tête absent, vide, erroné) partagent un code et un message publics indistinguables.
- **telegram** : client de l'API Bot écrit en direct sur `reqwest` — `getMe`, `sendMessage`,
  `sendChatAction`, `setWebhook`, `deleteWebhook`. Aucune URL n'apparaît dans une erreur ou un
  journal : le jeton du bot est un segment du chemin.
- **telegram** : découpage des messages sortants de plus de 4096 unités UTF-16, sous l'envoi
  plutôt qu'au-dessus, pour qu'aucun appelant ne puisse l'oublier. La coupe recule jusqu'au
  dernier saut de ligne, sinon la dernière espace, tant qu'elle ne sacrifie pas plus d'un quart
  du morceau.
- **worker** : file bornée à 256 places entre le webhook et la production des réponses. File
  pleine → `503` → Telegram rejoue : la contre-pression est déléguée à qui sait la gérer.
- **app** : extinction ordonnée — le serveur cesse de servir, le routeur est relâché, la file
  se vide, puis seulement le processus rend la main. Ce qui a été accusé est traité.
- **app** : `getMe` avant l'écoute. Un jeton révoqué ou mal collé fait échouer le **démarrage**,
  au lieu d'échouer silencieusement à chaque réponse.
- **config** : validation complète au démarrage, forme des secrets comprise. `Debug` masque le
  jeton et le secret de webhook.
- **error** : codes numériques stables `{"code": NNNN, "message": "..."}`, honorés aussi par les
  réponses du routeur et des couches `tower`. Numérotation partagée avec `agentbot` pour les
  codes communs.
- **exploitation** : `compagnon sonde`, `compagnon webhook declarer|retirer`, portés par le
  binaire livré — disponibles sur l'artefact, sans arbre source ni chaîne de compilation.
- **tests** : 25 tests unitaires, 12 de bout en bout et 1 doctest. Les tests e2e démarrent le
  vrai service sur une socket réelle et n'observent que ce que Telegram a reçu ; seule l'API
  Telegram est simulée, par `wiremock`, à la frontière HTTP sortante.
- **tests** : `tests/secrets.rs` regroupe les garanties d'absence — ce que le service ne doit
  pas faire. Cette classe de test se perd facilement : la version précédente du test de fuite
  couvrait exactement le complément du trou.
- **tests** : l'ordre « authentifier puis lire le corps » est éprouvé par un discriminant
  temporel — on annonce un corps qu'on n'envoie jamais, sur une socket brute. Une requête
  ordinaire ne distingue pas les deux ordres. Vérifié comme détectant bien la régression :
  3 s sans réponse sur l'ancien code, 239 µs sur le nouveau.
- **tests** : harnais réutilisable dans `tests/harnais/` — faux Telegram, service en marche sur
  port éphémère, attente d'appel avec compte-rendu d'échec. Les phases suivantes s'y greffent.

### Fixed

- **fix(telegram)** : **fuite du jeton du bot dans les journaux**. `ErreurEnvoi::Reseau` et
  `ErreurEnvoi::Illisible` conservaient un `reqwest::Error`, lequel transporte l'URL de l'appel
  et l'imprime dans son `Display` comme dans son `Debug` — or le jeton est un segment de cette
  URL. `worker::traiter` journalisant `%erreur` sur chaque envoi manqué, une simple coupure
  réseau écrivait le jeton dans `docker compose logs bot`, persisté sur disque. Les deux
  variantes ne portent plus qu'une `Panne` classée : le type n'a plus la capacité de porter une
  URL, y compris à travers un parcours de `source()`. Le test qui servait de filet construisait
  `ErreurEnvoi::Api`, seule variante qui ne pouvait pas fuir ; `tests/secrets.rs` éprouve
  désormais les trois, sur le vrai chemin contre un port fermé.
- **fix(http)** : le corps du webhook était **lu et alloué avant la vérification du secret**.
  Axum exécute les extracteurs — dont `Bytes`, qui draine la requête — puis seulement appelle le
  gestionnaire ; authentifier en première ligne du gestionnaire arrivait donc trop tard, et
  n'importe qui imposait la lecture de `TAILLE_MAX_CORPS` sur une adresse publique sans
  présenter de secret. La vérification vit dans une couche `route_layer` qui ne voit que les
  en-têtes, ce qui supprime au passage le clone intégral de la table d'en-têtes par requête.
  Conséquence assumée et testée : `GET /webhook` sans secret rend `401` et non `405`.
- **fix(telegram)** : `decouper` reparcourait tout le reste du texte à chaque tour, rendant la
  boucle quadratique — 1,19 s mesurées sur 4,5 millions d'unités UTF-16. Sans effet en phase 0,
  où l'entrant est plafonné, mais la sortie du modèle de la phase 1 ne le sera pas et le worker
  est à consommateur unique. Court-circuit en O(1) sur la longueur en octets.
- **fix(app)** : le vidage de la file à l'extinction était **non borné** alors que chaque
  `sendMessage` a un délai de 15 s : une file pleine face à un Telegram lent dépassait largement
  le sursis de Docker, et la perte était silencieuse. Vidage borné par `DELAI_VIDAGE` (25 s),
  aligné sur `stop_grace_period` (30 s), abandon journalisé en `ERROR`.

### Changed

- **change(error)** : une file pleine renvoie `5001` (`FileSaturee`) et non `9001` (`Interne`).
  Ce n'est pas une défaillance mais de la contre-pression, et le contrat public doit permettre
  de distinguer « le worker est saturé » de « le disque a lâché ». Statut `503` inchangé, pour
  que Telegram rejoue.
- **change(error)** : la couche d'enveloppe reconnaît une réponse déjà conforme à un marqueur
  privé au crate posé par `ApiError`, et non plus à son `content-type`. L'heuristique aurait été
  démentie en silence par un futur gestionnaire renvoyant son propre `Json(...)`.
- **change(http)** : `CatchPanicLayer` convertit une panique de gestionnaire en `9001` conforme.
  Sans lui, la connexion était coupée et Telegram voyait une réinitialisation au lieu d'un `5xx`.
- **change(telegram)** : `Ecart` porte son propre niveau de journal, comme `ErrorCode`. Le
  `match` de `webhook` finissait par un bras attrape-tout, qui aurait fait tomber en silence
  toute variante ajoutée par une phase suivante dans `debug!`. Le code d'erreur HTTP accolé à
  ces journaux est retiré : ces requêtes rendent `200`.
- **change(telegram)** : `Canal` et `Config` ne dérivent plus `Clone`, jamais utilisé et
  contradictoire — on interdit la copie du secret vers les journaux tout en autorisant la copie
  de la structure entière. `Recu` perd `Clone`, `PartialEq` et `Eq`, également inutilisés.
- **change(telegram)** : `Update::edited_message` est un `Option<IgnoredAny>` ; un
  `Option<Message>` faisait construire un message entier, allocations comprises, pour le jeter.
- **change(config)** : les validateurs ne rendent plus la valeur validée, ce qui faisait une
  seconde copie de chaque secret sur le tas.
- **change(http)** : `file_capacite` est lu sur le canal (`max_capacity`) et non recopié depuis
  la constante — les deux chiffres de la sonde viennent ainsi de la même source.

### Fixed

Trois défauts trouvés par la revue `/simplify` et vérifiés par la mesure avant correction —
tous trois introduits par les deux commits de cette phase.

- **fix(config)** : `masquer_url` **laissait fuir le mot de passe de la base**. Elle découpait
  l'URL sur `://` puis sur `@`, une grammaire devinée, et rendait la chaîne verbatim quand elle
  ne trouvait pas d'`@` — alors que sa documentation affirmait l'inverse. Deux formes que `sqlx`
  accepte imprimaient le mot de passe dans les journaux de démarrage : `postgres:///?password=…`
  et une URL dont le nom d'utilisateur contient une arobase. Le rendu est désormais reconstruit
  à partir des parties analysées par `sqlx` lui-même, donc exhaustif par construction. Cinq
  formes éprouvées dans `tests/secrets.rs`.
- **fix(db)** : la sérialisation par utilisateur **ne tenait que par chance**. Elle reposait sur
  un `pg_try_advisory_xact_lock` placé dans le `WHERE` — forme que PostgreSQL donne en
  contre-exemple, annotée « danger! ». Mesuré : 200 verrous posés pour réclamer une tâche, et
  une correction dépendante du plan (six workers servis avec un plan, **un seul** avec l'autre).
  La course qu'il prétendait fermer a été reproduite. L'invariant est désormais tenu par un
  index unique partiel, qui vaut quel que soit le plan et le niveau d'isolation.
- **fix(test)** : le test de la sonde **passait quoi que le service renvoie**. Il comparait deux
  champs supprimés par ce même commit ; `Value` indexé par une clé absente rend `Null`, donc
  l'assertion comparait `Null` à `Null`. `Sante` est maintenant réversible et le harnais la rend
  typée : un champ renommé devient une erreur de compilation.
- **fix(worker)** : le repos de 25 ms était payé après chaque **succès** — la moitié du cycle
  mesuré — alors qu'il est indispensable après un **échec**, où son absence fait épuiser les
  trois tentatives en quelques millisecondes.
- **fix(db)** : index manquant pour la borne par utilisateur, sur le chemin chaud du webhook.
  Mesuré : 1,963 ms à 50 000 tâches en attente, contre 0,044 ms avec l'index.
- **fix(db)** : `assurer` faisait mentir `mis_a_jour_le`. Un `do update` inconditionnel à chaque
  message déclenchait le trigger d'horodatage, faisant dire à la colonne « dernier message
  reçu » au lieu de « dernière modification » — c'est-à-dire exactement la colonne d'audit
  inutilisable que la migration dit vouloir éviter.
- **fix(test)** : la vérification d'âge, fonctionnalité vedette de cette phase, **n'avait aucun
  test**. Quatre tests l'écartaient en préambule, aucun ne l'éprouvait. Vérifié comme détectant
  bien sa suppression.

### Changed (revue)

- **change(worker)** : un type `Equipe` possède les consommateurs. Le lancement et l'extinction
  étaient recopiés dans les deux portes d'entrée et **avaient déjà divergé** le jour de leur
  écriture.
- **change(db)** : `Base::ouvrir` compose connexion et migration — « un `Base` existe » implique
  « son schéma est à jour », un fait de typage plutôt qu'une convention d'appel.
- **change(db)** : `ErreurBase::ChargeUtile` remplace un `sqlx::Error::Encode` détourné, qui
  annonçait « requête refusée par la base » pour une base qui n'avait rien reçu.
- **change(test)** : le harnais appelle `utilisateurs::verifier_age` au lieu de réécrire son
  SQL — la fonction de production n'avait aucun appelant pendant que sa copie tournait dans
  neuf tests, et les deux avaient déjà divergé.
- **change(http)** : la sonde annonce les consommateurs **encore en vie**, pas la constante.
- **change(db)** : `#[from]` sur `ErreurBase`, `query_scalar` et `FromRow` — seize recopies de
  `map_err(ErreurBase::Requete)` masquaient le SQL qu'elles entouraient.

### Removed

- `horloge::instant`, `http::adresse_liee`, `EtatApp::new` : trois items publics sans appelant
  ou délégant d'une ligne. `main.rs` perd son ordre supérieur générique et ses trois closures.

### Infrastructure

- **docker** : image à deux étages, exécution sans compilateur ni gestionnaire de paquets,
  utilisateur sans privilèges, `HEALTHCHECK` porté par le binaire.
- **proxy** : Caddy termine TLS — Telegram refuse un webhook qui n'est pas en HTTPS valide. Le
  journal du proxy **supprime** `X-Telegram-Bot-Api-Secret-Token` : Caddy ne caviarde d'office
  que `Cookie`, `Set-Cookie`, `Authorization` et `Proxy-Authorization`, et sans ce filtre le
  secret s'écrirait en clair à chaque message reçu.

### Notes

- La file vit en mémoire. L'extinction ordonnée la vide ; un arrêt brutal perd son contenu. La
  phase 1 la remplace par une file en base à bail.
- `tower-http` apparaît deux fois dans l'arbre (0.6 via `reqwest`, 0.7 en direct) : contrainte
  transitive, pas un choix.

## Version History

| Version | Date | Phase |
|---|---|---|
| 0.9.1 | 2026-09-05 | écritures partagées, filtre actif éprouvé |
| 0.9.0 | 2026-09-05 | revue 1.2 — garanties sur le texte |
| 0.8.0 | 2026-09-05 | 1.2e — création et exploitation |
| 0.7.0 | 2026-09-05 | 1.2d — modération |
| 0.6.0 | 2026-09-05 | 1.2c — composition du prompt |
| 0.5.0 | 2026-09-05 | 1.2b — tables du compagnon |
| 0.4.0 | 2026-09-05 | 1.2a — catalogues |
| 0.3.0 | 2026-09-05 | 1.1 — persistance |
| 0.2.2 | 2026-09-05 | 0 — modèle produit consigné |
| 0.2.1 | 2026-09-05 | 0 — décision « un seul bot » consignée |
| 0.2.0 | 2026-09-05 | 0 — réception par scrutation |
| 0.1.0 | 2026-09-05 | 0 — la boucle de transport Telegram |
