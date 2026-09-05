-- Noyau du schéma : utilisateur, compagnon, conversation, messages, file à bail.
--
-- Cardinalité : un utilisateur a UN compagnon et UNE conversation, imposé par des index
-- uniques partiels et non par une règle applicative. Il n'y a ni catalogue de personnages,
-- ni bascule entre plusieurs, ni notion de « personnage actif ».
--
-- Les tables de détail `personnage_*` (apparence, archétypes, curseurs, prompt) relèvent de
-- la phase 1.2. Seule la table centrale est créée ici, parce que `conversations` la référence.

-- ---------------------------------------------------------------------------
-- Horodatage de modification, tenu par la base
-- ---------------------------------------------------------------------------
-- Une fonction unique plutôt qu'un `set mis_a_jour_le = now()` répété dans chaque requête :
-- un seul oubli applicatif rendrait la colonne mensongère, et une colonne d'audit à laquelle
-- on ne peut pas se fier ne vaut rien.
create or replace function toucher_mis_a_jour_le() returns trigger as $$
begin
    new.mis_a_jour_le = now();
    return new;
end;
$$ language plpgsql;

-- ---------------------------------------------------------------------------
-- Utilisateurs
-- ---------------------------------------------------------------------------
create table utilisateurs (
    -- L'identifiant Telegram, jamais généré ici : il est stable, unique, et déjà connu du
    -- premier message. En fabriquer un second créerait deux identités pour une personne.
    id                              bigint primary key,
    prenom_affiche                  text,
    langue                          text not null default 'fr',
    fuseau_horaire                  text not null default 'Europe/Paris',
    code_pays_declare               text,

    -- Vérification d'âge : sans elle, aucun accès au moteur de dialogue, dès cette phase.
    age_verifie_le                  timestamptz,
    methode_verification_age        text
                                    check (methode_verification_age in
                                           ('declaration', 'prestataire_tiers', 'document')),
    reference_verification_externe  text,

    -- Consentements : état courant ici, historique dans `historique_consentement`.
    consentement_suggestions_commerciales   boolean not null default false,
    consentement_contenu_suggestif_proactif boolean not null default false,
    intensite_suggestive_choisie            numeric(3,2) not null default 0.00
                                    check (intensite_suggestive_choisie between 0.00 and 1.00),

    onboarding_termine_le           timestamptz,
    cree_le                         timestamptz not null default now(),
    mis_a_jour_le                   timestamptz not null default now(),
    supprime_le                     timestamptz,

    -- Une méthode de vérification sans date, ou l'inverse, décrirait un état impossible.
    constraint verification_age_coherente check (
        (age_verifie_le is null and methode_verification_age is null)
        or (age_verifie_le is not null and methode_verification_age is not null)
    )
);

create trigger trg_utilisateurs_touch
    before update on utilisateurs
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Compagnons
-- ---------------------------------------------------------------------------
create table personnages (
    id                  uuid primary key default gen_random_uuid(),
    utilisateur_id      bigint not null references utilisateurs(id),
    nom                 text not null,
    statut              text not null default 'brouillon'
                        check (statut in ('brouillon', 'actif', 'rejete')),
    version             integer not null default 1,
    cree_le             timestamptz not null default now(),
    mis_a_jour_le       timestamptz not null default now(),
    supprime_le         timestamptz
);

-- Un compagnon par utilisateur. Partiel sur `supprime_le is null` pour qu'une suppression
-- douce laisse la place à un nouveau sans détruire l'ancien, qui reste auditable.
create unique index idx_un_compagnon_par_utilisateur
    on personnages (utilisateur_id) where supprime_le is null;

create trigger trg_personnages_touch
    before update on personnages
    for each row execute function toucher_mis_a_jour_le();

-- ---------------------------------------------------------------------------
-- Conversations
-- ---------------------------------------------------------------------------
-- Distincte de `personnages` bien que liée un pour un : celle-ci porte l'état du fil
-- (dernier échange, puis résumés et souvenirs en phase 2), celle-là l'identité du compagnon.
-- Deux cycles de vie — modifier les traits du compagnon ne doit pas toucher la mémoire.
create table conversations (
    id                  uuid primary key default gen_random_uuid(),
    utilisateur_id      bigint not null references utilisateurs(id),
    personnage_id       uuid not null references personnages(id),
    dernier_message_le  timestamptz,
    cree_le             timestamptz not null default now(),
    supprime_le         timestamptz
);

create unique index idx_une_conversation_par_utilisateur
    on conversations (utilisateur_id) where supprime_le is null;
create unique index idx_une_conversation_par_personnage
    on conversations (personnage_id) where supprime_le is null;

-- ---------------------------------------------------------------------------
-- Messages
-- ---------------------------------------------------------------------------
create table messages (
    id                      uuid primary key default gen_random_uuid(),
    conversation_id         uuid not null references conversations(id),
    role                    text not null check (role in ('utilisateur', 'personnage')),
    modalite                text not null default 'texte'
                            check (modalite in ('texte', 'image', 'audio')),
    contenu                 text,
    -- Clé de stockage objet, jamais l'octet lui-même : une base ne doit pas devenir un
    -- entrepôt de médias, et une purge RGPD doit pouvoir viser le fichier séparément.
    reference_media         text,
    origine                 text not null default 'reponse'
                            check (origine in ('reponse', 'proactif')),
    identifiant_telegram    bigint,
    cree_le                 timestamptz not null default now()
);

create index idx_messages_conversation_date
    on messages (conversation_id, cree_le desc);

-- ---------------------------------------------------------------------------
-- Historique des consentements — append-only
-- ---------------------------------------------------------------------------
create table historique_consentement (
    id              uuid primary key default gen_random_uuid(),
    utilisateur_id  bigint not null references utilisateurs(id),
    type            text not null
                    check (type in ('suggestions_commerciales', 'contenu_suggestif_proactif')),
    valeur          boolean not null,
    canal           text not null
                    check (canal in ('onboarding', 'reglages', 'commande_bot')),
    modifie_le      timestamptz not null default now()
);

create index idx_consentement_utilisateur_type
    on historique_consentement (utilisateur_id, type, modifie_le desc);

-- ---------------------------------------------------------------------------
-- File de traitement à bail
-- ---------------------------------------------------------------------------
-- Remplace la file en mémoire de la phase 0, dont le contenu disparaissait à tout arrêt
-- brutal. Le bail, plutôt qu'un état « en cours » nu, est ce qui rend une tâche récupérable
-- quand le worker qui la tenait meurt sans la rendre.
create table file_messages (
    id                  uuid primary key default gen_random_uuid(),
    utilisateur_id      bigint not null references utilisateurs(id),
    charge_utile        jsonb not null,
    type_tache          text not null
                        check (type_tache in ('message_entrant', 'proactif', 'generation_image',
                                              'extraction_signaux', 'compaction_memoire')),
    statut              text not null default 'en_attente'
                        check (statut in ('en_attente', 'en_cours', 'traite', 'echec')),
    bail_expire_le      timestamptz,
    tentatives          smallint not null default 0,
    -- Un CODE d'erreur stable, jamais le `Display` d'une erreur : c'est exactement le chemin
    -- par lequel un jeton fuirait, et ce chemin a déjà été emprunté une fois dans ce projet.
    erreur_derniere     integer,
    cree_le             timestamptz not null default now(),
    traite_le           timestamptz
);

-- Index de prise de tâche : ne couvre que ce qui est prenable, donc il reste petit même
-- quand la table accumule des lignes traitées.
create index idx_file_a_prendre
    on file_messages (cree_le)
    where statut in ('en_attente', 'en_cours');

-- Sert la sous-requête « cet utilisateur a-t-il déjà une tâche en vol ? », évaluée à chaque
-- prise de tâche. Mesuré : le planificateur s'en sert pour `bail_expire_le >= now()` et
-- applique l'égalité d'utilisateur en filtre de jointure — la colonne de tête ne travaille
-- donc pas. Sans conséquence tant que l'ensemble des tâches en vol est borné par le nombre de
-- workers, ce qui est le cas ; à revoir si ce nombre grandit d'un ordre de grandeur.
create index idx_file_en_cours_par_utilisateur
    on file_messages (utilisateur_id, bail_expire_le)
    where statut = 'en_cours';
