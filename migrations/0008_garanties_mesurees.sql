-- Trois garanties que la phase 1.3 affirmait et ne tenait pas. Chacune mesurée avant correctif.
--
-- # Le motif, encore
--
-- La migration 0006 l'énonçait déjà : « la garantie portait sur une forme, jamais sur le texte ».
-- Ici c'est une variante — la garantie porte sur les chemins auxquels on a pensé, et pas sur
-- ceux qu'on n'a pas nommés. Un trigger `for each row` couvre `insert`, `update`, `delete`, et
-- **jamais** `truncate`. Une contrainte écrite pour un sens ne dit rien de l'autre.
--
-- Les trois trous ont été reproduits sur un PostgreSQL réel avant d'être bouchés.

-- ---------------------------------------------------------------------------
-- 1. Le registre se vidait par « truncate »
-- ---------------------------------------------------------------------------
-- Mesuré, sur la base migrée jusqu'à 0007 :
--
--     delete from consommation;    -- ERROR: une ligne de coût ne se supprime pas
--     truncate consommation;       -- TRUNCATE TABLE       → 0 ligne restante
--
-- Un trigger de ligne n'est jamais appelé par `truncate`, qui ne parcourt aucune ligne. La
-- table annoncée « append-only » se vidait donc d'une instruction plus courte que celle qui
-- était refusée.
create or replace function consommation_refuse_le_vidage() returns trigger as $$
begin
    raise exception
        'consommation est un registre : la table ne se vide pas'
        using errcode = 'restrict_violation',
              hint = 'pour une purge RGPD, détacher les lignes (utilisateur_id à null)';
end;
$$ language plpgsql;

create trigger consommation_immuable_vidage
    before truncate on consommation
    for each statement execute function consommation_refuse_le_vidage();

-- ---------------------------------------------------------------------------
-- 2. Une ligne pouvait NAÎTRE orpheline
-- ---------------------------------------------------------------------------
-- La migration 0007 affirme que sa contrainte « interdit d'insérer une ligne sans utilisateur ».
-- C'est faux : `(anonymisee_le is null) = (utilisateur_id is not null)` est satisfaite par le
-- couple (anonymisee_le renseigné, utilisateur_id nul). Mesuré :
--
--     insert into consommation (utilisateur_id, anonymisee_le, …, cout_fournisseur_eur, …)
--     values (null, now(), …, 9.999999, …);        -- INSERT 0 1
--
-- N'importe quel montant pouvait donc entrer au registre sans être imputable à personne, ce qui
-- est exactement ce que le passage de `not null` à nullable prétendait rendre impossible.
--
-- La distinction juste n'est pas un état mais une **transition** : une ligne naît rattachée,
-- l'anonymisation est ce qui la détache ensuite. Un `check` ne peut pas l'exprimer — il ne voit
-- qu'une ligne, jamais son histoire —, d'où le trigger.
create or replace function consommation_nait_rattachee() returns trigger as $$
begin
    if new.anonymisee_le is not null then
        raise exception
            'consommation : une ligne naît rattachée à un utilisateur ; l''anonymisation est '
            'une transition, jamais un état initial'
            using errcode = 'restrict_violation';
    end if;
    return new;
end;
$$ language plpgsql;

create trigger consommation_rattachee_a_la_naissance
    before insert on consommation
    for each row execute function consommation_nait_rattachee();

-- ---------------------------------------------------------------------------
-- 3. La table qui porte le texte modéré était la seule exclue de la doctrine de 0006
-- ---------------------------------------------------------------------------
-- La migration 0006 pose : toute modification d'un compagnon révoque sa validation. Elle
-- l'applique aux cinq tables de traits et au nom. Elle **n'a pas** été appliquée à
-- `personnage_parametres_modele`, c'est-à-dire à la seule table qui contient le texte que la
-- modération a réellement examiné.
--
-- `trg_validation_retiree` ne s'y déclenche que si `valide_le` devient nul. Réécrire
-- `prompt_systeme_genere` en gardant `valide_le` traversait donc tout l'appareil : statut actif
-- conservé, aucune révocation, aucune version inscrite.
--
-- Le chemin légitime — `personnage::valider` — réhorodate `valide_le` dans la même instruction
-- que le prompt. C'est ce qui permet de séparer les deux sans nommer le code appelant : un
-- prompt qui change sans que sa validation soit réémise n'est plus validé.
--
-- Ce que cela NE ferme PAS, et il faut le dire : une écriture qui pose `valide_le = now()` en
-- même temps que le texte passe. L'empreinte vivant dans la même ligne, elle peut être
-- recalculée de la même main. Seul un sceau dont la clé n'est pas dans la base — un HMAC —
-- fermerait ce chemin, et c'est une décision de déploiement, pas de migration.
create or replace function revoquer_sur_changement_de_prompt() returns trigger as $$
begin
    if (new.prompt_systeme_genere is distinct from old.prompt_systeme_genere
        or new.prompt_systeme_hash is distinct from old.prompt_systeme_hash)
       and new.valide_le is not distinct from old.valide_le
    then
        new.valide_le := null;
    end if;
    return new;
end;
$$ language plpgsql;

create trigger trg_prompt_revoque
    before update on personnage_parametres_modele
    for each row execute function revoquer_sur_changement_de_prompt();

-- ---------------------------------------------------------------------------
-- 4. Le message entrant était réinscrit à chaque reprise
-- ---------------------------------------------------------------------------
-- Mesuré : un modèle qui expire fait rejouer la tâche trois fois, et `messages` finit avec
-- trois copies de ce que la personne a écrit une seule fois. Rien ne l'interdisait —
-- `identifiant_telegram` est nullable et n'était couvert par aucun index.
--
-- La conséquence immédiate est mineure (deux transactions de trop). Celle de la phase 2 ne
-- l'est pas : c'est cette table qui composera l'historique envoyé au modèle, et un incident
-- réseau d'aujourd'hui deviendrait un tour de conversation dupliqué des semaines plus tard.
--
-- Telegram fournit déjà la clé d'idempotence : l'identifiant du message. L'index est partiel
-- parce que les messages du compagnon n'en ont pas tant qu'ils ne sont pas partis.
create unique index idx_message_unique_par_identifiant_telegram
    on messages (conversation_id, identifiant_telegram)
    where identifiant_telegram is not null;
