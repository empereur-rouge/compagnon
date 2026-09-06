-- Le registre des coûts : une ligne par appel payant, et personne ne peut la réécrire.
--
-- # Pourquoi dès maintenant, avant tout abonnement
--
-- La roadmap fixe les quotas en 1.6, et le réflexe serait d'attendre la table jusque-là. Ce
-- serait fixer les paliers sur une estimation. Cette table existe pour que la question « combien
-- coûte réellement un utilisateur actif par mois » ait une réponse **mesurée** le jour où il
-- faudra y répondre — et cette mesure ne peut pas être reconstituée après coup.
--
-- # Pourquoi elle est immuable
--
-- C'est un registre comptable. Un coût qui peut être modifié après écriture ne répond plus à la
-- question qu'il sert à répondre : la marge se calcule sur ce qui a été payé, pas sur ce qu'on
-- se souvient avoir payé. Aucune ligne de code n'a de raison légitime de faire un `update` ou un
-- `delete` ici, donc la table n'en offre pas la possibilité — même motif que la migration 0006,
-- où le texte éditorial a cessé d'être modifiable plutôt que de rester modifiable-mais-interdit.
--
-- Une exception, et une seule : la purge RGPD, qui doit détacher la ligne de son utilisateur
-- sans en perdre le montant (`SCHEMA-NOYAU.md` : « conserver uniquement ce que la comptabilité
-- impose, sous forme anonymisée »). Elle est admise par le trigger, décrite exactement, et rien
-- d'autre ne l'est.

-- ---------------------------------------------------------------------------
-- 1. La table
-- ---------------------------------------------------------------------------
create table consommation (
    id                      uuid primary key default gen_random_uuid(),

    -- Les trois rattachements. Nullables **ensemble**, et seulement une fois la ligne
    -- anonymisée : voir la contrainte `consommation_anonymisation_coherente` plus bas, qui
    -- interdit d'insérer une ligne sans utilisateur. Le schéma d'origine les voulait `not null`
    -- ; ils ne peuvent pas l'être si la purge doit détacher au lieu de supprimer.
    utilisateur_id          bigint references utilisateurs(id),
    conversation_id         uuid references conversations(id),
    -- Null si la génération a échoué avant qu'un message existe.
    message_id              uuid references messages(id),

    type                    text not null
                            check (type in ('message', 'image', 'audio', 'extraction', 'compaction')),
    origine                 text not null
                            check (origine in ('reponse', 'proactif', 'tache_fond')),

    -- 'runpod', 'replicate', 'elevenlabs'… Texte libre assumé : c'est un nom d'hébergeur, il
    -- n'entre dans aucun prompt et ne sert qu'à regrouper des montants.
    fournisseur             text not null check (fournisseur <> ''),
    -- L'identifiant EXACT rendu par le fournisseur, pas celui demandé. Les deux diffèrent dès
    -- qu'il y a un alias ou une bascule de version, et comparer le coût de deux versions sur ce
    -- qu'on croyait appeler ne compare rien.
    modele                  text not null check (modele <> ''),

    -- Jetons d'entrée / jetons de sortie, ou secondes de GPU, ou secondes d'audio selon le
    -- `type`. Null quand le fournisseur ne les rend pas.
    unites_entree           integer check (unites_entree >= 0),
    unites_sortie           integer check (unites_sortie >= 0),

    -- `numeric(10,6)` : les coûts unitaires sont de l'ordre du millième d'euro. En virgule
    -- flottante, la somme d'un million de lignes dérive — et c'est précisément la somme d'un
    -- million de lignes qu'on vient chercher ici.
    cout_fournisseur_eur    numeric(10,6) not null check (cout_fournisseur_eur >= 0),

    -- Mesurée par l'appelant, pas annoncée par le fournisseur : une latence annoncée exclut la
    -- file d'attente et le réseau, c'est-à-dire l'essentiel de ce que l'utilisateur ressent.
    duree_ms                integer check (duree_ms >= 0),

    statut                  text not null check (statut in ('ok', 'echec', 'rejete_moderation')),
    cree_le                 timestamptz not null default now(),

    -- Quand la purge RGPD a détaché la ligne. Null tant qu'elle est rattachée.
    anonymisee_le           timestamptz,

    -- Les deux moitiés se gardent l'une l'autre : une ligne rattachée n'a pas de date
    -- d'anonymisation, une ligne anonymisée n'a plus d'utilisateur.
    --
    -- ATTENTION — cette contrainte ne dit RIEN de l'insertion. Le couple (anonymisee_le
    -- renseigné, utilisateur_id nul) la satisfait, et une ligne pouvait donc naître orpheline
    -- avec n'importe quel montant. Mesuré, et corrigé par la migration 0008 : c'est un trigger
    -- `before insert` qui l'interdit, parce que la distinction est une transition et non un
    -- état, ce qu'un `check` ne peut pas voir.
    constraint consommation_anonymisation_coherente check (
        (anonymisee_le is null) = (utilisateur_id is not null)
    )
);

comment on table consommation is
    'Registre append-only des coûts fournisseur. Une ligne par appel payant. '
    'Seule mutation admise : l''anonymisation RGPD, qui détache sans effacer le montant.';

create index idx_consommation_utilisateur_periode
    on consommation (utilisateur_id, cree_le desc);
create index idx_consommation_type_date
    on consommation (type, cree_le desc);

-- ---------------------------------------------------------------------------
-- 2. L'immuabilité
-- ---------------------------------------------------------------------------
-- La comparaison porte sur la ligne ENTIÈRE, pas sur une liste de colonnes. C'est délibéré :
-- une liste devrait être tenue à jour à chaque colonne ajoutée, et une liste oubliée est une
-- garantie qui s'éteint en silence. Ici, toute colonne future est couverte le jour où elle
-- apparaît, sans que personne n'ait à y penser.
create or replace function consommation_est_un_registre() returns trigger as $$
declare
    apres_anonymisation consommation%rowtype;
begin
    if tg_op = 'DELETE' then
        raise exception
            'consommation est un registre : une ligne de coût ne se supprime pas'
            using errcode = 'restrict_violation',
                  hint = 'pour une purge RGPD, détacher la ligne (utilisateur_id à null) '
                         'plutôt que la supprimer';
    end if;

    -- La seule forme d'`update` admise, construite depuis l'ancienne ligne : les trois
    -- rattachements à null, l'horodatage d'anonymisation posé. Tout le reste identique.
    apres_anonymisation := old;
    apres_anonymisation.utilisateur_id  := null;
    apres_anonymisation.conversation_id := null;
    apres_anonymisation.message_id      := null;
    apres_anonymisation.anonymisee_le   := new.anonymisee_le;

    if old.anonymisee_le is not null then
        raise exception
            'consommation : cette ligne est déjà anonymisée, elle ne se modifie plus'
            using errcode = 'restrict_violation';
    end if;

    if new.anonymisee_le is null then
        raise exception
            'consommation : la seule mise à jour admise est l''anonymisation RGPD'
            using errcode = 'restrict_violation',
                  hint = 'poser anonymisee_le et mettre les trois rattachements à null';
    end if;

    if new is distinct from apres_anonymisation then
        raise exception
            'consommation : une anonymisation détache la ligne, elle ne change rien d''autre'
            using errcode = 'restrict_violation',
                  hint = 'seuls utilisateur_id, conversation_id, message_id et anonymisee_le '
                         'peuvent changer, et uniquement vers null / une date';
    end if;

    return new;
end;
$$ language plpgsql;

create trigger consommation_immuable
    before update or delete on consommation
    for each row execute function consommation_est_un_registre();
