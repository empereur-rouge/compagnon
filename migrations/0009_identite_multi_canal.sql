-- L'identité de l'utilisateur cesse d'être celle de Telegram.
--
-- # Pourquoi maintenant, alors que Telegram est le seul canal
--
-- Parce que c'est la dernière fenêtre où ça ne coûte presque rien. `SCHEMA-API.md` et la
-- révision de `SCHEMA-NOYAU.md` demandent cette bascule « dès la phase 1.1 » ; la phase 1.5
-- ajoutera l'onboarding et la 1.6 les abonnements et les quotas, c'est-à-dire deux familles de
-- tables de plus indexées par utilisateur, et de vraies données dedans.
--
-- Le choix précédent — `utilisateurs.id` = identifiant Telegram — était défendable et défendu :
-- il est stable, unique, connu dès le premier message, et n'invente pas une seconde identité.
-- Ce qu'il ne permet pas, c'est qu'une même personne existe sur deux canaux. Le jour où sortir
-- de Telegram devient une décision plutôt qu'une option, l'identifiant du canal cesse d'être une
-- identité : il redevient ce qu'il est, une adresse.
--
-- # Ce que la bascule préserve
--
-- Tout. Chaque utilisateur existant reçoit un UUID, et son identifiant Telegram descend dans
-- `identifiants_externes` où il garde son unicité. Aucune ligne n'est perdue, aucun compagnon
-- n'est détaché, aucune conversation ne change de propriétaire — les jointures ci-dessous le
-- garantissent, et les tests de la phase 1.3 le vérifient après coup.

-- ---------------------------------------------------------------------------
-- 1. La nouvelle identité, à côté de l'ancienne
-- ---------------------------------------------------------------------------
-- Pas de contrainte d'unicité transitoire sur cette colonne : les clés étrangères qui la
-- viseraient s'y accrocheraient, et il faudrait les défaire pour la remplacer par la clé
-- primaire. Elles sont donc toutes posées à la fin, section 5, quand la bascule est faite.
alter table utilisateurs add column identite uuid not null default gen_random_uuid();

-- ---------------------------------------------------------------------------
-- 2. Le pont vers les canaux
-- ---------------------------------------------------------------------------
-- La résolution `(canal, identifiant_externe) → utilisateur_id` est le premier traitement de
-- toute requête entrante, quel que soit le canal. C'est le SEUL endroit du schéma, avec
-- `messages.identifiant_telegram`, où un identifiant de canal a le droit d'apparaître.
create table identifiants_externes (
    id                  uuid primary key default gen_random_uuid(),
    utilisateur_id      uuid not null,
    canal               text not null check (canal in ('telegram', 'api', 'web')),
    -- En texte, et non en nombre : un identifiant de canal n'est pas une quantité, et le
    -- prochain canal n'aura aucune raison d'en fournir un numérique.
    identifiant_externe text not null check (identifiant_externe <> ''),
    cree_le             timestamptz not null default now(),
    -- Deux personnes ne peuvent pas partager une adresse sur un même canal.
    unique (canal, identifiant_externe)
);

-- Un utilisateur peut porter plusieurs canaux, mais pas deux fois le même : sans cet index, une
-- personne pourrait accumuler des adresses Telegram et le service ne saurait plus par laquelle
-- lui répondre.
create unique index idx_un_identifiant_par_canal_et_utilisateur
    on identifiants_externes (utilisateur_id, canal);

create index idx_identifiants_par_utilisateur on identifiants_externes (utilisateur_id);

-- L'existant descend dans le pont.
insert into identifiants_externes (utilisateur_id, canal, identifiant_externe)
select identite, 'telegram', id::text from utilisateurs;

-- ---------------------------------------------------------------------------
-- 3. Chaque table dépendante suit
-- ---------------------------------------------------------------------------
-- Le motif est le même six fois : ajouter la colonne, la remplir par jointure, la rendre
-- obligatoire, puis basculer contraintes et index. Écrit à plat plutôt qu'en boucle
-- dynamique : une migration se relit, et un `execute format(...)` sur des noms de contraintes
-- cache exactement ce qu'un relecteur vient vérifier.

-- personnages
--
-- Le trigger d'horodatage est suspendu le temps du remplissage : sans cela, `mis_a_jour_le`
-- dirait « migré le » au lieu de « dernière modification », et la migration 0001 justifie
-- précisément ce trigger par « une colonne d'audit à laquelle on ne peut pas se fier ne vaut
-- rien ». Une bascule d'identité n'est pas une modification du compagnon.
alter table personnages disable trigger trg_personnages_touch;
alter table personnages add column utilisateur uuid;
update personnages p set utilisateur = u.identite from utilisateurs u where u.id = p.utilisateur_id;
alter table conversations drop constraint conversations_compagnon_du_meme_utilisateur;
alter table personnages drop constraint personnages_id_utilisateur;
alter table personnages drop constraint personnages_utilisateur_id_fkey;
drop index idx_un_compagnon_par_utilisateur;
alter table personnages drop column utilisateur_id;
alter table personnages rename column utilisateur to utilisateur_id;
alter table personnages alter column utilisateur_id set not null;
alter table personnages add constraint personnages_id_utilisateur unique (id, utilisateur_id);
create unique index idx_un_compagnon_par_utilisateur
    on personnages (utilisateur_id) where supprime_le is null;
alter table personnages enable trigger trg_personnages_touch;

-- conversations
alter table conversations add column utilisateur uuid;
update conversations c set utilisateur = u.identite from utilisateurs u where u.id = c.utilisateur_id;
alter table conversations drop constraint conversations_utilisateur_id_fkey;
drop index idx_une_conversation_par_utilisateur;
alter table conversations drop column utilisateur_id;
alter table conversations rename column utilisateur to utilisateur_id;
alter table conversations alter column utilisateur_id set not null;
create unique index idx_une_conversation_par_utilisateur
    on conversations (utilisateur_id) where supprime_le is null;
-- Le triangle se referme comme avant : la conversation d'un utilisateur pointe le compagnon DE
-- CET utilisateur, et c'est une clé composite qui le rend inconstructible (migration 0004).
alter table conversations add constraint conversations_compagnon_du_meme_utilisateur
    foreign key (personnage_id, utilisateur_id) references personnages (id, utilisateur_id);

-- historique_consentement
alter table historique_consentement add column utilisateur uuid;
update historique_consentement h set utilisateur = u.identite from utilisateurs u where u.id = h.utilisateur_id;
alter table historique_consentement drop constraint historique_consentement_utilisateur_id_fkey;
drop index idx_consentement_utilisateur_type;
alter table historique_consentement drop column utilisateur_id;
alter table historique_consentement rename column utilisateur to utilisateur_id;
alter table historique_consentement alter column utilisateur_id set not null;
create index idx_consentement_utilisateur_type
    on historique_consentement (utilisateur_id, type, modifie_le desc);

-- file_messages
alter table file_messages add column utilisateur uuid;
update file_messages f set utilisateur = u.identite from utilisateurs u where u.id = f.utilisateur_id;
alter table file_messages drop constraint file_messages_utilisateur_id_fkey;
drop index idx_une_tache_en_vol_par_utilisateur;
drop index idx_file_en_cours_par_utilisateur;
drop index idx_file_en_file_par_utilisateur;
alter table file_messages drop column utilisateur_id;
alter table file_messages rename column utilisateur to utilisateur_id;
alter table file_messages alter column utilisateur_id set not null;
-- L'index unique partiel EST le mécanisme qui borne la file à une tâche en vol par utilisateur
-- (migration 0002). Le recréer à l'identique n'est pas une formalité : sans lui, la requête de
-- prise laisse deux workers servir la même personne, et les réponses se croisent.
create unique index idx_une_tache_en_vol_par_utilisateur
    on file_messages (utilisateur_id) where statut = 'en_cours';
create index idx_file_en_cours_par_utilisateur
    on file_messages (utilisateur_id, bail_expire_le) where statut = 'en_cours';
create index idx_file_en_file_par_utilisateur
    on file_messages (utilisateur_id) where statut in ('en_attente', 'en_cours');

-- personnage_historique_versions
alter table personnage_historique_versions add column modifie_par_uuid uuid;
update personnage_historique_versions h set modifie_par_uuid = u.identite
  from utilisateurs u where u.id = h.modifie_par;
alter table personnage_historique_versions drop constraint personnage_historique_versions_modifie_par_fkey;
alter table personnage_historique_versions drop column modifie_par;
alter table personnage_historique_versions rename column modifie_par_uuid to modifie_par;

-- consommation
--
-- Le registre refuse la migration, et c'est le comportement voulu : le trigger de la migration
-- 0007 n'admet qu'une seule forme d'`update`, l'anonymisation RGPD. Remplir une colonne neuve
-- n'en est pas une, et l'essai le confirme :
--
--     ERROR: consommation : la seule mise à jour admise est l'anonymisation RGPD
--
-- Le trigger est donc retiré puis reposé à l'identique, autour du seul remplissage. Ce n'est pas
-- un contournement mais le régime que la migration 0006 a énoncé pour le texte éditorial : ce
-- qui est immuable l'est **hors migration**. Une migration se relit, se date et se versionne ;
-- une console `psql` non. Et le passage ci-dessus vaut démonstration que la garantie porte.
--
-- La colonne est nullable, contrairement aux autres : c'est celle que la purge RGPD détache.
drop trigger consommation_immuable on consommation;

alter table consommation add column utilisateur uuid;
update consommation c set utilisateur = u.identite from utilisateurs u where u.id = c.utilisateur_id;
alter table consommation drop constraint consommation_utilisateur_id_fkey;
drop index idx_consommation_utilisateur_periode;
alter table consommation drop column utilisateur_id;
alter table consommation rename column utilisateur to utilisateur_id;
create index idx_consommation_utilisateur_periode
    on consommation (utilisateur_id, cree_le desc);

create trigger consommation_immuable
    before update or delete on consommation
    for each row execute function consommation_est_un_registre();

-- ---------------------------------------------------------------------------
-- 4. L'ancienne identité disparaît
-- ---------------------------------------------------------------------------
-- Le pont créé plus haut pointait `utilisateurs(identite)` ; une fois `identite` devenue la clé
-- primaire sous le nom `id`, les six clés étrangères suivent le renommage sans rien perdre.
alter table utilisateurs drop constraint utilisateurs_pkey;
alter table utilisateurs drop column id;
alter table utilisateurs rename column identite to id;
alter table utilisateurs add primary key (id);
alter table utilisateurs alter column id set default gen_random_uuid();

-- ---------------------------------------------------------------------------
-- 5. Les clés étrangères, posées d'un coup sur la clé primaire définitive
-- ---------------------------------------------------------------------------
-- Groupées ici plutôt que dispersées dans la section 3 : posées plus tôt, elles auraient visé
-- une contrainte d'unicité transitoire, et il aurait fallu les défaire pour la remplacer.
-- PostgreSQL refuse d'ailleurs de supprimer un index dont une clé étrangère dépend, ce qui est
-- exactement le garde-fou attendu.
alter table identifiants_externes add constraint identifiants_externes_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id) on delete cascade;
alter table personnages add constraint personnages_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id);
alter table conversations add constraint conversations_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id);
alter table historique_consentement add constraint historique_consentement_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id);
alter table file_messages add constraint file_messages_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id);
alter table personnage_historique_versions add constraint personnage_historique_versions_modifie_par_fkey
    foreign key (modifie_par) references utilisateurs(id);
alter table consommation add constraint consommation_utilisateur_id_fkey
    foreign key (utilisateur_id) references utilisateurs(id);

comment on table identifiants_externes is
    'Le pont (canal, identifiant externe) → utilisateur. Seul endroit du schéma, avec '
    'messages.identifiant_telegram, où un identifiant de canal a le droit d''apparaître.';
