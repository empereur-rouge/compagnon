-- Les vocabulaires contrôlés dans lesquels l'utilisateur choisit.
--
-- # Le principe qui justifie toutes ces tables
--
-- L'utilisateur ne rédige jamais le prompt système : il choisit des valeurs dans ces
-- catalogues, et le service compose. Trois conséquences, dont la première est la seule qui
-- compte vraiment :
--
-- 1. **La modération porte sur le catalogue, une fois, pas sur chaque compagnon créé.** Si
--    aucune valeur possible n'évoque un mineur, aucune composition ne le peut. C'est une
--    garantie structurelle, pas un filtre qu'on espère fiable.
-- 2. Les données sont exploitables : quels archétypes retiennent, quelle intensité convertit.
-- 3. Retirer une option, c'est passer un `actif` à faux — pas auditer du texte libre.
--
-- Le peuplement vit dans cette migration et non dans un script à part : ces valeurs ne sont pas
-- des données d'exemple, ce sont des **constantes du produit** dont le code dépend. Une base
-- migrée sans elles ne pourrait créer aucun compagnon.

-- ---------------------------------------------------------------------------
-- Apparence
-- ---------------------------------------------------------------------------
create table ref_genres (
    id          uuid primary key default gen_random_uuid(),
    code        text not null unique,
    libelle     text not null,
    actif       boolean not null default true,
    cree_le     timestamptz not null default now()
);

create table ref_morphologies (like ref_genres including all);
create table ref_couleurs_cheveux (like ref_genres including all);
create table ref_couleurs_yeux (like ref_genres including all);
create table ref_styles_vestimentaires (like ref_genres including all);

-- Tranche d'âge apparente : **aucune valeur sous 25 ans, et la base l'impose**.
--
-- La contrainte n'est pas une ceinture de sécurité sur des données déjà correctes : c'est le
-- point où l'interdiction absolue du projet cesse d'être une règle qu'on applique pour devenir
-- une forme que la base refuse. Une ligne à 17 ans est rejetée à l'écriture, quel que soit le
-- chemin — code Rust, console psql, restauration partielle.
--
-- Le plancher est à 25 et non à 18 : une apparence « jeune adulte » proche de la limite est
-- exactement la zone grise qu'aucun classifieur ne tranche de façon fiable, et que ce produit
-- n'a aucune raison d'explorer.
create table ref_tranches_age_apparent (
    id          uuid primary key default gen_random_uuid(),
    code        text not null unique,
    libelle     text not null,
    age_min     smallint not null check (age_min >= 25),
    actif       boolean not null default true,
    cree_le     timestamptz not null default now()
);

insert into ref_genres (code, libelle) values
    ('femme', 'Femme'), ('homme', 'Homme'), ('non_binaire', 'Non binaire');

insert into ref_morphologies (code, libelle) values
    ('mince', 'Mince'), ('athletique', 'Athlétique'), ('pulpeuse', 'Pulpeuse'),
    ('ronde', 'Ronde'), ('musclee', 'Musclée'), ('elancee', 'Élancée');

insert into ref_couleurs_cheveux (code, libelle) values
    ('brun', 'Bruns'), ('blond', 'Blonds'), ('roux', 'Roux'), ('noir', 'Noirs'),
    ('chatain', 'Châtains'), ('gris', 'Gris'), ('colore', 'Colorés');

insert into ref_couleurs_yeux (code, libelle) values
    ('marron', 'Marron'), ('bleu', 'Bleus'), ('vert', 'Verts'),
    ('noisette', 'Noisette'), ('gris', 'Gris'), ('noir', 'Noirs');

insert into ref_styles_vestimentaires (code, libelle) values
    ('decontracte', 'Décontracté'), ('elegant', 'Élégant'), ('sportif', 'Sportif'),
    ('bohème', 'Bohème'), ('rock', 'Rock'), ('classique', 'Classique'),
    ('minimaliste', 'Minimaliste');

insert into ref_tranches_age_apparent (code, libelle, age_min) values
    ('25_34', '25 à 34 ans', 25), ('35_44', '35 à 44 ans', 35), ('45_plus', '45 ans et plus', 45);

-- ---------------------------------------------------------------------------
-- Archétypes, et leurs fusions nommées
-- ---------------------------------------------------------------------------
create table ref_archetypes (
    id          uuid primary key default gen_random_uuid(),
    code        text not null unique,
    libelle     text not null,
    -- Injectée telle quelle dans le prompt système : c'est du texte éditorial, écrit ici et
    -- validé une fois, jamais saisi par un utilisateur.
    description text not null,
    actif       boolean not null default true,
    cree_le     timestamptz not null default now()
);

insert into ref_archetypes (code, libelle, description) values
    ('chaleureux', 'Chaleureux', 'accueillant et bienveillant, met à l''aise sans effort'),
    ('joueur', 'Joueur', 'taquin, aime les jeux de mots et les piques affectueuses'),
    ('protecteur', 'Protecteur', 'attentif au bien-être de son interlocuteur, rassurant'),
    ('timide', 'Timide', 'réservé au premier abord, se livre peu à peu'),
    ('dominant', 'Dominant', 'assuré, mène la conversation, sait ce qu''il veut'),
    ('intellectuel', 'Intellectuel', 'curieux, aime les idées et les conversations de fond'),
    ('romantique', 'Romantique', 'sensible, attentif aux sentiments et aux détails'),
    ('mysterieux', 'Mystérieux', 'se dévoile lentement, garde une part d''ombre'),
    ('rebelle', 'Rebelle', 'indocile, se méfie des conventions'),
    ('nourricier', 'Nourricier', 'prend soin, materne, veille'),
    ('ambitieux', 'Ambitieux', 'porté par ses projets, entreprenant'),
    ('reveur', 'Rêveur', 'imaginatif, la tête ailleurs, poétique'),
    ('sarcastique', 'Sarcastique', 'ironique, l''humour comme distance'),
    ('loyal', 'Loyal', 'fidèle, constant, sur qui on peut compter'),
    ('independant', 'Indépendant', 'tient à son autonomie, ne s''accroche pas'),
    ('vulnerable', 'Vulnérable', 'montre ses failles, ne se protège pas'),
    ('charismatique', 'Charismatique', 'magnétique, on l''écoute'),
    ('calme', 'Calme', 'posé, difficile à déstabiliser'),
    ('impulsif', 'Impulsif', 'réagit vite, se laisse porter par l''instant'),
    ('possessif', 'Possessif', 'attaché de près, supporte mal le partage');

-- La fusion est **orientée** : principal → secondaire.
--
-- « Principalement timide avec une pointe de dominance » n'est pas « principalement dominant
-- avec une pointe de timidité ». Le yandere, c'est le premier. Deux lignes distinctes si les
-- deux sens ont chacun un sens narratif.
create table ref_fusions_archetypes (
    id                  uuid primary key default gen_random_uuid(),
    code_principal      text not null references ref_archetypes(code),
    code_secondaire     text not null references ref_archetypes(code),
    nom_fusion          text not null,
    -- Remplace la simple concaténation des deux descriptions dans le prompt.
    description_fusion  text not null,
    actif               boolean not null default true,
    unique (code_principal, code_secondaire),
    -- Une fusion d'un archétype avec lui-même n'a pas de sens et fausserait la résolution.
    constraint fusion_entre_deux_archetypes_distincts check (code_principal <> code_secondaire)
);

insert into ref_fusions_archetypes (code_principal, code_secondaire, nom_fusion, description_fusion) values
    ('timide', 'dominant', 'Yandere',
     'réservé en surface et étonnamment affirmé dès qu''il s''agit de ce qui lui tient à cœur'),
    ('protecteur', 'possessif', 'Garde du corps',
     'veille de près, jusqu''à une attention qui frôle la jalousie'),
    ('chaleureux', 'mysterieux', 'Envoûtant',
     'accueillant et pourtant insaisissable, on ne sait jamais tout de lui'),
    ('romantique', 'impulsif', 'Passionné',
     'les sentiments à fleur de peau, agit avant de réfléchir'),
    ('calme', 'sarcastique', 'Flegmatique',
     'imperturbable, l''ironie posée sans jamais hausser le ton'),
    ('vulnerable', 'charismatique', 'Fragile-magnétique',
     'montre ses failles et c''est précisément ce qui attire'),
    ('intellectuel', 'joueur', 'Espiègle érudit',
     'la culture portée avec légèreté, le savoir comme terrain de jeu'),
    ('independant', 'romantique', 'Libre et attaché',
     'tient à son autonomie et aime pourtant profondément');

-- ---------------------------------------------------------------------------
-- Tons, même mécanique
-- ---------------------------------------------------------------------------
create table ref_tons (like ref_archetypes including all);

insert into ref_tons (code, libelle, description) values
    ('formel', 'Formel', 'vouvoie, phrases construites, distance courtoise'),
    ('familier', 'Familier', 'tutoie, langage de tous les jours'),
    ('humoristique', 'Humoristique', 'cherche le rire, ne se prend pas au sérieux'),
    ('romantique', 'Romantique', 'mots choisis, attention portée à l''autre'),
    ('direct', 'Direct', 'va droit au but, sans détour'),
    ('poetique', 'Poétique', 'images, métaphores, rythme'),
    ('sarcastique', 'Sarcastique', 'second degré permanent'),
    ('tendre', 'Tendre', 'doux, enveloppant'),
    ('brut', 'Brut', 'sans filtre, cru'),
    ('taquin', 'Taquin', 'cherche la réaction, gentiment'),
    ('theatral', 'Théâtral', 'emphase, grands gestes'),
    ('autoritaire', 'Autoritaire', 'affirme, ne demande pas'),
    ('nonchalant', 'Nonchalant', 'détaché, rien ne presse');

create table ref_fusions_tons (
    id                  uuid primary key default gen_random_uuid(),
    code_principal      text not null references ref_tons(code),
    code_secondaire     text not null references ref_tons(code),
    nom_fusion          text not null,
    description_fusion  text not null,
    actif               boolean not null default true,
    unique (code_principal, code_secondaire),
    constraint fusion_entre_deux_tons_distincts check (code_principal <> code_secondaire)
);

insert into ref_fusions_tons (code_principal, code_secondaire, nom_fusion, description_fusion) values
    ('tendre', 'autoritaire', 'Possessif tendre', 'doux mais ne laisse pas de place au doute'),
    ('romantique', 'direct', 'Sans détour', 'dit ce qu''il ressent, sans emballage'),
    ('humoristique', 'sarcastique', 'Piquant', 'drôle et mordant à la fois'),
    ('formel', 'taquin', 'Décalé', 'la politesse comme terrain de jeu'),
    ('poetique', 'brut', 'Âpre', 'des images, mais sans joliesse');

-- ---------------------------------------------------------------------------
-- Paramètres graduels, et leurs plafonds par pays
-- ---------------------------------------------------------------------------
-- `numeric(3,2)` et non `real` : deux décimales exactes, sans les approximations binaires des
-- flottants. Un `real` pourrait stocker 0,373829… par arrondi ; ici c'est structurellement
-- impossible, plutôt que dépendant d'une validation applicative.
create table ref_parametres_gradues (
    id                      uuid primary key default gen_random_uuid(),
    code                    text not null unique,
    libelle                 text not null,
    domaine                 text not null
                            check (domaine in ('personnalite', 'contenu', 'proactivite')),
    valeur_min              numeric(3,2) not null default 0.00,
    valeur_max              numeric(3,2) not null default 1.00,
    valeur_defaut           numeric(3,2) not null,
    plafonnable_juridiction boolean not null default false,
    actif                   boolean not null default true,
    cree_le                 timestamptz not null default now(),
    constraint bornes_coherentes check (valeur_min <= valeur_defaut and valeur_defaut <= valeur_max)
);

insert into ref_parametres_gradues (code, libelle, domaine, valeur_defaut, plafonnable_juridiction) values
    ('humour',               'Humour',                'personnalite', 0.50, false),
    ('affection',            'Affection',             'personnalite', 0.50, false),
    ('assurance',            'Assurance',             'personnalite', 0.50, false),
    ('intensite_suggestive', 'Intensité suggestive',  'contenu',      0.00, true),
    ('frequence_proactive',  'Fréquence des messages spontanés', 'proactivite', 0.30, false),
    ('seuil_absence_jours',  'Délai avant reprise de contact',    'proactivite', 0.50, false);

-- Plafonds par juridiction.
--
-- Le pays est **déclaratif**, saisi à l'inscription — jamais déduit d'une géolocalisation. Cela
-- évite de construire un système de profilage de localisation, et c'est cohérent avec la
-- vérification d'âge.
--
-- Cette table doit être peuplée AVANT toute ouverture d'un pays : c'est un processus
-- opérationnel avec revue légale, pas un champ technique laissé vide par défaut. Vide, elle ne
-- plafonne rien — ce qui est le bon défaut pour un pays qu'on n'a pas encore examiné, à
-- condition de ne pas y ouvrir le service.
create table ref_plafonds_juridiction (
    id              uuid primary key default gen_random_uuid(),
    code_pays       text not null,
    parametre_code  text not null references ref_parametres_gradues(code),
    valeur_max      numeric(3,2) not null check (valeur_max between 0.00 and 1.00),
    source_legale   text,
    mis_a_jour_le   timestamptz not null default now(),
    unique (code_pays, parametre_code)
);
