-- Les termes qu'un nom de compagnon ne peut pas contenir.
--
-- # Pourquoi une table et pas une constante Rust
--
-- Même raison que pour les catalogues : retirer ou ajouter un terme doit être une écriture, pas
-- un déploiement. Un signalement arrive un dimanche soir ; attendre une recompilation pour y
-- répondre serait absurde.
--
-- # Ce que cette liste protège, et ce qu'elle ne protège pas
--
-- Le nom est le SEUL texte libre d'un compagnon : tout le reste vient de catalogues clos, donc
-- une composition ne peut pas évoquer un mineur par ses traits. Cette liste couvre l'unique
-- interstice qui reste.
--
-- Elle ne le couvre pas complètement, et il faut le dire : une liste de termes rate les
-- graphies détournées, les diminutifs, les langues qu'elle ne contient pas. C'est une première
-- ligne, pas un classifieur — le vrai contrôle arrive avec le client de modèle en phase 1.3,
-- où le nom pourra être soumis avec son contexte. Ce qui est structurel ici, c'est que le nom
-- soit le seul endroit à examiner ; ce qui est heuristique, c'est l'examen lui-même.
create table ref_termes_interdits (
    id          uuid primary key default gen_random_uuid(),
    terme       text not null unique,
    -- Ce que le terme évoque : sert à mesurer ce que la liste attrape, et à retirer une
    -- catégorie entière si elle se révèle trop large.
    motif       text not null check (motif in ('mineur', 'famille_proche', 'personne_reelle')),
    actif       boolean not null default true,
    cree_le     timestamptz not null default now()
);

create index idx_termes_actifs on ref_termes_interdits (terme) where actif;

-- Termes évoquant un mineur. Le rapprochement se fait sur un nom NORMALISÉ — minuscules, sans
-- accents, sans séparateurs — pour que « Petite-Fille » et « petitefille » soient tous deux
-- attrapés.
insert into ref_termes_interdits (terme, motif) values
    ('enfant', 'mineur'), ('enfants', 'mineur'),
    ('bebe', 'mineur'), ('bebes', 'mineur'), ('baby', 'mineur'),
    ('gamin', 'mineur'), ('gamine', 'mineur'),
    ('gosse', 'mineur'), ('mome', 'mineur'),
    ('fillette', 'mineur'), ('garconnet', 'mineur'),
    ('petitefille', 'mineur'), ('petitgarcon', 'mineur'),
    ('ado', 'mineur'), ('adolescent', 'mineur'), ('adolescente', 'mineur'),
    ('teen', 'mineur'), ('teenager', 'mineur'), ('preteen', 'mineur'),
    ('lycéenne', 'mineur'), ('lyceenne', 'mineur'), ('lyceen', 'mineur'),
    ('collegienne', 'mineur'), ('collegien', 'mineur'),
    ('ecoliere', 'mineur'), ('ecolier', 'mineur'), ('schoolgirl', 'mineur'),
    ('mineur', 'mineur'), ('mineure', 'mineur'), ('loli', 'mineur'), ('shota', 'mineur'),
    ('nymphette', 'mineur'), ('jailbait', 'mineur'), ('underage', 'mineur'),
    ('puceau', 'mineur'), ('pucelle', 'mineur'), ('lolita', 'mineur');

-- Les mentions d'âge — « lea12ans », « 15yo » — ne sont pas dans cette liste : elles sont
-- couvertes structurellement par le refus de tout chiffre dans un nom. Une liste aurait dû
-- énumérer les graphies ; l'interdiction les couvre toutes, et un nom de compagnon n'a de
-- toute façon aucune raison de contenir un chiffre.

-- Termes de parenté proche. Motif distinct : ce n'est pas la même interdiction, et les
-- confondre empêcherait d'en retirer une sans l'autre.
insert into ref_termes_interdits (terme, motif) values
    ('maman', 'famille_proche'), ('papa', 'famille_proche'),
    ('mere', 'famille_proche'), ('pere', 'famille_proche'),
    ('soeur', 'famille_proche'), ('frere', 'famille_proche'),
    ('fille', 'famille_proche'), ('fils', 'famille_proche'),
    ('mommy', 'famille_proche'), ('daddy', 'famille_proche'),
    ('sister', 'famille_proche'), ('brother', 'famille_proche'),
    ('stepsister', 'famille_proche'), ('stepbrother', 'famille_proche'),
    ('stepdaughter', 'famille_proche'), ('stepson', 'famille_proche');
