-- Confie à la base l'invariant « au plus une tâche en vol par utilisateur ».
--
-- # Ce que remplace cet index
--
-- La requête de prise tenait cet invariant par une composition de trois mécanismes, dont un
-- `pg_try_advisory_xact_lock(utilisateur_id)` placé dans le `WHERE`. Trois défauts, tous
-- mesurés plutôt que supposés :
--
-- 1. **La documentation PostgreSQL donne cette forme en contre-exemple**, annotée « danger! » :
--    une fonction de verrouillage dans un `WHERE` avec `LIMIT` n'est pas garantie d'être
--    évaluée après la limite. Mesuré sur ce schéma : 200 verrous posés pour réclamer UNE
--    tâche, tous tenus jusqu'au commit.
-- 2. **La correction dépendait du plan.** Sur six workers concurrents, mêmes données, même
--    requête : plan pipeliné → 6 servis ; plan avec tri → 1 servi, les cinq autres affamés.
--    Une mise à jour de statistiques suffisait à basculer de l'un à l'autre, sans erreur.
-- 3. **La course n'était pas fermée.** En `read committed`, deux workers dont les instantanés
--    précèdent le commit de l'autre voient tous deux le `not exists` vrai. Reproduit : deux
--    tâches du même utilisateur prises simultanément — donc l'ordre dans une conversation,
--    que le service promet, ne tenait que par chance.
--
-- # Pourquoi un index unique partiel
--
-- L'unicité est vérifiée à l'insertion de l'entrée d'index, sous verrou de page, hors MVCC :
-- c'est le seul mécanisme qui tienne quel que soit le plan, le niveau d'isolation et l'ordre
-- d'évaluation des clauses. C'est aussi celui que ce schéma emploie déjà pour la cardinalité
-- des compagnons et des conversations — la file était le seul endroit où la concurrence mord
-- réellement, et le seul parti sur une règle applicative.
--
-- Une prise concurrente sur le même utilisateur reçoit désormais une violation d'unicité
-- (`23505`), que le worker traite comme « rien à prendre » et rejoue. Le `not exists` de la
-- requête demeure, mais comme filtre d'efficacité — il évite la collision — et non plus comme
-- mécanisme de correction.
create unique index idx_une_tache_en_vol_par_utilisateur
    on file_messages (utilisateur_id) where statut = 'en_cours';

-- Un état impossible doit être impossible, comme pour `verification_age_coherente`.
--
-- Sans cette contrainte, une ligne `en_cours` sans échéance est acceptée par la base et
-- devient invisible DES DEUX CÔTÉS de la requête de prise : `bail_expire_le < now()` est faux
-- sur NULL, et `bail_expire_le >= now()` aussi. Elle n'est donc ni prenable ni bloquante —
-- elle disparaît. Aucun chemin Rust ne la produit aujourd'hui ; une reprise manuelle
-- d'incident ou un futur module la produiront.
alter table file_messages
    add constraint bail_coherent
    check ((statut = 'en_cours') = (bail_expire_le is not null));

-- Sert le comptage de la borne par utilisateur, sur le chemin chaud du webhook.
--
-- Sans lui, `file::enfiler` balaie tout l'arriéré et le filtre : mesuré à 1,963 ms avec
-- 50 000 tâches en attente, contre 0,044 ms avec (44×), et 4 buffers au lieu d'un millier.
-- Le coût devient celui de la file de CET utilisateur — donc borné par
-- EN_FILE_MAX_PAR_UTILISATEUR, constant quel que soit l'arriéré.
--
-- L'arriéré est nul aujourd'hui, où la réponse est un écho. La phase 1.3 est faite pour en
-- créer un : quatre workers divisés par trois secondes d'appel de modèle font 1,3 tâche par
-- seconde, et tout ce qui arrive plus vite s'accumule.
create index idx_file_en_file_par_utilisateur
    on file_messages (utilisateur_id)
    where statut in ('en_attente', 'en_cours');
