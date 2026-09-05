-- Les traits d'un compagnon, et ce que la base refuse d'en faire.
--
-- Chaque table ne porte qu'un aspect, et toutes pendent de `personnages` par une clé primaire
-- qui est aussi une clé étrangère : un compagnon a au plus une apparence, un jeu de paramètres,
-- un prompt. La forme rend le doublon impossible plutôt que surveillé.

-- ---------------------------------------------------------------------------
-- Apparence
-- ---------------------------------------------------------------------------
create table personnage_apparence (
    personnage_id           uuid primary key references personnages(id),
    genre_id                uuid not null references ref_genres(id),
    -- `not null` : c'est le champ par lequel une apparence mineure entrerait, et l'omettre ne
    -- doit pas être un moyen de contourner le catalogue.
    tranche_age_id          uuid not null references ref_tranches_age_apparent(id),
    morphologie_id          uuid not null references ref_morphologies(id),
    couleur_cheveux_id      uuid references ref_couleurs_cheveux(id),
    longueur_cheveux        text check (longueur_cheveux in ('courts', 'mi_longs', 'longs')),
    couleur_yeux_id         uuid references ref_couleurs_yeux(id),
    style_vestimentaire_id  uuid references ref_styles_vestimentaires(id),
    -- Ancre visuelle de la phase 3. Rien ne l'écrit encore.
    graine_visuelle         text,
    mis_a_jour_le           timestamptz not null default now()
);

create trigger trg_apparence_touch before update on personnage_apparence
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Personnalité : archétypes et tons, un principal et jusqu'à deux secondaires
-- ---------------------------------------------------------------------------
create table personnage_archetypes (
    personnage_id   uuid not null references personnages(id),
    archetype_id    uuid not null references ref_archetypes(id),
    role            text not null check (role in ('principal', 'secondaire')),
    -- Ordre des secondaires ; nul pour le principal, et la contrainte l'impose dans les deux
    -- sens : un principal rangé, ou un secondaire sans rang, décriraient un état que la
    -- résolution du prompt ne saurait pas lire.
    rang            smallint check (rang in (1, 2)),
    primary key (personnage_id, archetype_id),
    constraint rang_coherent_avec_role check (
        (role = 'principal' and rang is null) or (role = 'secondaire' and rang is not null)
    )
);

-- Les deux règles du document, imposées à l'écriture et non en validation applicative : une
-- tentative d'insérer un second principal ou un troisième secondaire échoue.
create unique index idx_un_seul_archetype_principal
    on personnage_archetypes (personnage_id) where role = 'principal';
create unique index idx_archetypes_secondaires_rang_unique
    on personnage_archetypes (personnage_id, rang) where role = 'secondaire';

create table personnage_tons (
    personnage_id   uuid not null references personnages(id),
    ton_id          uuid not null references ref_tons(id),
    role            text not null check (role in ('principal', 'secondaire')),
    rang            smallint check (rang in (1, 2)),
    primary key (personnage_id, ton_id),
    constraint rang_ton_coherent_avec_role check (
        (role = 'principal' and rang is null) or (role = 'secondaire' and rang is not null)
    )
);

create unique index idx_un_seul_ton_principal
    on personnage_tons (personnage_id) where role = 'principal';
create unique index idx_tons_secondaires_rang_unique
    on personnage_tons (personnage_id, rang) where role = 'secondaire';

-- ---------------------------------------------------------------------------
-- Curseurs
-- ---------------------------------------------------------------------------
create table personnage_parametres_gradues (
    personnage_id   uuid not null references personnages(id),
    parametre_code  text not null references ref_parametres_gradues(code),
    valeur          numeric(3,2) not null,
    mis_a_jour_le   timestamptz not null default now(),
    primary key (personnage_id, parametre_code),
    constraint valeur_dans_bornes check (valeur between 0.00 and 1.00)
);

create trigger trg_gradues_touch before update on personnage_parametres_gradues
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Interaction
-- ---------------------------------------------------------------------------
create table personnage_parametres_interaction (
    personnage_id        uuid primary key references personnages(id),
    -- Fenêtre dans laquelle le compagnon peut initier. Le fuseau est celui de l'UTILISATEUR
    -- (`utilisateurs.fuseau_horaire`), pas du compagnon : c'est l'humain qui dort.
    plage_horaire_debut  time,
    plage_horaire_fin    time,
    longueur_reponse     text not null default 'moyenne'
                         check (longueur_reponse in ('courte', 'moyenne', 'longue')),
    mis_a_jour_le        timestamptz not null default now(),
    -- Une borne sans l'autre décrirait une fenêtre qu'aucun code ne saurait interpréter.
    constraint plage_complete_ou_absente check (
        (plage_horaire_debut is null) = (plage_horaire_fin is null)
    )
);

create trigger trg_interaction_touch before update on personnage_parametres_interaction
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Le prompt système — généré, jamais saisi
-- ---------------------------------------------------------------------------
create table personnage_parametres_modele (
    personnage_id           uuid primary key references personnages(id),
    -- Composé par le service à partir des champs structurés. Aucun chemin ne permet à un
    -- utilisateur d'y écrire : c'est le point de contrôle unique de la modération.
    prompt_systeme_genere   text not null,
    -- Empreinte du texte ci-dessus, pour détecter un écart introduit hors du processus —
    -- une console psql, une restauration partielle, un script d'exploitation.
    prompt_systeme_hash     text not null,
    modele_cible            text not null,
    temperature             numeric(3,2) not null default 0.80,
    top_p                   numeric(3,2) not null default 0.90,
    version_prompt          integer not null default 1,
    -- Rempli SEULEMENT après passage de la modération.
    valide_le               timestamptz,
    mis_a_jour_le           timestamptz not null default now(),
    constraint temperature_plausible check (temperature between 0.00 and 2.00),
    constraint top_p_plausible check (top_p between 0.00 and 1.00)
);

create trigger trg_modele_touch before update on personnage_parametres_modele
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Le verrou d'activation
-- ---------------------------------------------------------------------------
-- La spécification dit : « `valide_le` nul ⇒ le personnage ne peut pas passer en
-- `statut = 'actif'`. Vérifiable en base par une requête d'audit. »
--
-- Vérifiable n'est pas tenu. Une garantie qu'on peut constater après coup est une garantie que
-- rien n'empêche d'enfreindre, et celle-ci est la dernière avant qu'un compagnon ne se mette à
-- parler : elle porte tout ce que la modération aura décidé.
--
-- Une contrainte `check` ne peut pas lire une autre table ; un déclencheur, si. Celui-ci refuse
-- l'activation quel que soit le chemin d'écriture — code Rust, console, script.
create or replace function refuser_activation_sans_validation() returns trigger as $$
begin
    if new.statut = 'actif' and (old.statut is distinct from 'actif') then
        if not exists (
            select 1 from personnage_parametres_modele
            where personnage_id = new.id and valide_le is not null
        ) then
            raise exception 'compagnon % : activation refusée, prompt non validé', new.id
                using errcode = 'check_violation';
        end if;
    end if;
    return new;
end;
$$ language plpgsql;

create trigger trg_activation_exige_validation
    before insert or update of statut on personnages
    for each row execute function refuser_activation_sans_validation();

-- ---------------------------------------------------------------------------
-- Historique — journal daté, append-only
-- ---------------------------------------------------------------------------
create table personnage_historique_versions (
    id              uuid primary key default gen_random_uuid(),
    personnage_id   uuid not null references personnages(id),
    version         integer not null,
    modifie_le      timestamptz not null default now(),
    modifie_par     bigint not null references utilisateurs(id),
    raison          text not null
                    check (raison in ('creation', 'mise_a_jour_utilisateur',
                                      'moderation_rejet', 'moderation_validation',
                                      'suppression')),
    -- Instantané de TOUTES les tables `personnage_*`, plus le prompt résultant. C'est ce qui
    -- permet de répondre à « à quoi ressemblait ce compagnon le 3 mars » sans reconstituer.
    etat_complet    jsonb not null,
    unique (personnage_id, version)
);

create index idx_historique_personnage_version
    on personnage_historique_versions (personnage_id, version desc);

-- ---------------------------------------------------------------------------
-- Le triangle utilisateur / compagnon / conversation
-- ---------------------------------------------------------------------------
-- Trois index uniques garantissaient trois bornes indépendantes — un compagnon par utilisateur,
-- une conversation par utilisateur, une conversation par compagnon — mais aucun ne garantissait
-- que la conversation d'un utilisateur pointe le compagnon DE CET utilisateur. Les deux clés
-- étrangères de `conversations` étaient disjointes : une insertion directe pouvait relier
-- l'utilisateur A au compagnon de B.
--
-- Une clé composite rend le triangle inconstructible, au lieu de le garder en trois endroits.
alter table personnages add constraint personnages_id_utilisateur unique (id, utilisateur_id);

alter table conversations drop constraint conversations_personnage_id_fkey;
alter table conversations
    add constraint conversations_compagnon_du_meme_utilisateur
    foreign key (personnage_id, utilisateur_id) references personnages (id, utilisateur_id);
