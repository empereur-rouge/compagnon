-- Ferme trois trous du même motif : la garantie portait sur une forme, jamais sur le texte.
--
-- # Le motif, énoncé une fois
--
-- La phase 1.2 revendique une sûreté structurelle : « si aucune valeur du catalogue n'évoque un
-- mineur, aucune composition ne le peut ». C'était faux, et d'une façon qui ne se voyait pas à
-- la lecture — chaque garantie s'arrêtait au moment précis où elle aurait dû porter sur du
-- texte plutôt que sur une forme.
--
-- Reproduit avant correction, en une seule écriture :
--
--     update ref_tranches_age_apparent set libelle = 'Adolescente de 16 ans' where code='25_34';
--
-- `check (age_min >= 25)` satisfait, test au vert, modération acceptée — et le prompt envoyé au
-- modèle contenait « Femme, Adolescente de 16 ans ». La contrainte gardait `age_min`, une
-- colonne que rien ne lit ; `libelle`, le seul champ qui atteigne le modèle, n'en avait aucune.

-- ---------------------------------------------------------------------------
-- 1. Le texte éditorial devient immuable hors migration
-- ---------------------------------------------------------------------------
-- Les descriptions du catalogue sont injectées telles quelles dans le prompt. « Écrit et validé
-- une fois » décrivait la relecture d'une migration, pas une propriété de la table : après
-- déploiement, un `update` atteignait toute composition future sans passer par aucun verdict,
-- aucune empreinte, aucun historique.
--
-- La difficulté est qu'on ne peut pas simplement figer ces tables : le produit REVENDIQUE de
-- pouvoir retirer une option à chaud — c'est l'argument de `ref_termes_interdits`, « un
-- signalement arrive un dimanche soir ». La mutabilité voulue pour la liste noire était la
-- faille de la liste blanche.
--
-- On sépare donc les deux régimes plutôt que les deux intentions : `actif` reste modifiable,
-- le texte non. Le retrait rétroactif garde toute sa force, l'altération silencieuse disparaît.
create or replace function refuser_alteration_editoriale() returns trigger as $$
declare
    colonne text;
begin
    foreach colonne in array tg_argv loop
        if to_jsonb(new) ->> colonne is distinct from to_jsonb(old) ->> colonne then
            raise exception
                'catalogue %.% : le texte éditorial ne se modifie que par migration (colonne « % »)',
                tg_table_schema, tg_table_name, colonne
                using errcode = 'check_violation',
                      hint = 'pour retirer une option sans déploiement, passer « actif » à faux';
        end if;
    end loop;
    return new;
end;
$$ language plpgsql;

create trigger trg_genres_editorial before update on ref_genres
    for each row execute function refuser_alteration_editoriale('libelle');
create trigger trg_morphologies_editorial before update on ref_morphologies
    for each row execute function refuser_alteration_editoriale('libelle');
create trigger trg_cheveux_editorial before update on ref_couleurs_cheveux
    for each row execute function refuser_alteration_editoriale('libelle');
create trigger trg_yeux_editorial before update on ref_couleurs_yeux
    for each row execute function refuser_alteration_editoriale('libelle');
create trigger trg_styles_editorial before update on ref_styles_vestimentaires
    for each row execute function refuser_alteration_editoriale('libelle');
create trigger trg_tranches_editorial before update on ref_tranches_age_apparent
    for each row execute function refuser_alteration_editoriale('libelle', 'age_min');
create trigger trg_archetypes_editorial before update on ref_archetypes
    for each row execute function refuser_alteration_editoriale('libelle', 'description');
create trigger trg_tons_editorial before update on ref_tons
    for each row execute function refuser_alteration_editoriale('libelle', 'description');
create trigger trg_fusions_arch_editorial before update on ref_fusions_archetypes
    for each row execute function refuser_alteration_editoriale('nom_fusion', 'description_fusion');
create trigger trg_fusions_tons_editorial before update on ref_fusions_tons
    for each row execute function refuser_alteration_editoriale('nom_fusion', 'description_fusion');

-- ---------------------------------------------------------------------------
-- 2. Le plafond de juridiction cesse de porter sur un proxy
-- ---------------------------------------------------------------------------
-- `charger_curseurs` filtrait `domaine = 'personnalite'` et joignait les plafonds sans regarder
-- `plafonnable_juridiction`. Or le seul paramètre marqué plafonnable est `intensite_suggestive`,
-- de domaine `contenu` : les plafonds ne pouvaient s'appliquer qu'à des paramètres déclarés NON
-- plafonnables, et jamais à celui pour lequel le mécanisme existe. Le drapeau, le commentaire du
-- schéma et le code disaient trois choses différentes.
--
-- Le correctif côté Rust est de joindre sur `plafonnable_juridiction`. Reste à nommer ce que
-- `domaine` servait à dire — « ce curseur entre dans le prompt du compagnon » — au lieu de le
-- déduire.
alter table ref_parametres_gradues
    add column entre_dans_le_prompt boolean not null default false,
    -- Qui porte le curseur. `intensite_suggestive` est porté par l'UTILISATEUR et non par le
    -- compagnon : c'est un choix de l'humain sur ce qu'il veut recevoir, pas un trait. Sans
    -- cette colonne, la création en écrivait une copie sur le compagnon, créant deux sources de
    -- vérité pour le seul paramètre à conséquence légale.
    add column porte_par text not null default 'compagnon'
        check (porte_par in ('compagnon', 'utilisateur'));

update ref_parametres_gradues set entre_dans_le_prompt = true
 where code in ('humour', 'affection', 'assurance');
update ref_parametres_gradues set porte_par = 'utilisateur'
 where code = 'intensite_suggestive';

-- Un curseur porté par l'utilisateur n'a rien à faire sur un compagnon. Rendu inexprimable
-- plutôt que déconseillé.
create or replace function refuser_curseur_de_l_utilisateur() returns trigger as $$
begin
    if exists (select 1 from ref_parametres_gradues
                where code = new.parametre_code and porte_par = 'utilisateur') then
        raise exception
            'le curseur « % » est porté par l''utilisateur, pas par le compagnon',
            new.parametre_code
            using errcode = 'check_violation';
    end if;
    return new;
end;
$$ language plpgsql;

create trigger trg_curseur_du_compagnon
    before insert or update on personnage_parametres_gradues
    for each row execute function refuser_curseur_de_l_utilisateur();

delete from personnage_parametres_gradues
 where parametre_code in (select code from ref_parametres_gradues where porte_par = 'utilisateur');

-- ---------------------------------------------------------------------------
-- 3. La validation devient un état que toute modification révoque
-- ---------------------------------------------------------------------------
-- Le verrou d'activation ne gardait que l'INSTANT de la transition. Après validation, les
-- traits et le nom restaient librement modifiables : un compagnon pouvait rester `actif` en
-- portant un prompt validé qui ne le décrivait plus, et un nom jamais modéré. C'était le second
-- chemin par lequel du texte non modéré atteignait le modèle — et un test du dépôt faisait
-- exactement cette manœuvre, en la prenant pour un montage anodin.
create or replace function revoquer_la_validation() returns trigger as $$
declare
    cible uuid := coalesce(new.personnage_id, old.personnage_id);
begin
    update personnage_parametres_modele set valide_le = null where personnage_id = cible;
    update personnages set statut = 'brouillon' where id = cible and statut = 'actif';
    return coalesce(new, old);
end;
$$ language plpgsql;

create trigger trg_apparence_revoque after insert or update or delete on personnage_apparence
    for each row execute function revoquer_la_validation();
create trigger trg_archetypes_revoque after insert or update or delete on personnage_archetypes
    for each row execute function revoquer_la_validation();
create trigger trg_tons_revoque after insert or update or delete on personnage_tons
    for each row execute function revoquer_la_validation();
create trigger trg_gradues_revoque after insert or update or delete on personnage_parametres_gradues
    for each row execute function revoquer_la_validation();
create trigger trg_interaction_revoque after insert or update or delete on personnage_parametres_interaction
    for each row execute function revoquer_la_validation();

-- Le nom est le seul texte libre : le changer après modération est la manœuvre la plus directe.
--
-- En DEUX déclencheurs, et l'ordre importe. Écrit d'un seul tenant, le `before` modifiait
-- `personnage_parametres_modele`, dont le déclencheur revenait modifier la ligne `personnages`
-- en cours d'écriture — PostgreSQL refuse (« tuple to be updated was already modified »).
--
-- Scindé, la séquence se déroule : le `before` rabat le statut dans `new`, la ligne est écrite,
-- puis l'`after` retire la validation. Le déclencheur symétrique s'exécute alors sur une ligne
-- déjà en `brouillon` et ne touche rien.
create or replace function rabattre_le_statut_sur_changement_de_nom() returns trigger as $$
begin
    if new.nom is distinct from old.nom and new.statut = 'actif' then
        new.statut := 'brouillon';
    end if;
    return new;
end;
$$ language plpgsql;

create trigger trg_nom_rabat_statut before update of nom on personnages
    for each row execute function rabattre_le_statut_sur_changement_de_nom();

create or replace function revoquer_sur_changement_de_nom() returns trigger as $$
begin
    if new.nom is distinct from old.nom then
        update personnage_parametres_modele set valide_le = null where personnage_id = new.id;
    end if;
    return new;
end;
$$ language plpgsql;

create trigger trg_nom_revoque after update of nom on personnages
    for each row execute function revoquer_sur_changement_de_nom();

-- Et le côté symétrique, qui manquait : retirer la validation d'un compagnon actif le rabat en
-- brouillon plutôt que de le laisser actif sans prompt validé. L'invariant « actif ⇒ prompt
-- validé » était gardé sur une table et pas sur l'autre.
create or replace function rabattre_si_validation_retiree() returns trigger as $$
begin
    if tg_op = 'DELETE' or new.valide_le is null then
        update personnages set statut = 'brouillon'
         where id = coalesce(new.personnage_id, old.personnage_id) and statut = 'actif';
    end if;
    return coalesce(new, old);
end;
$$ language plpgsql;

create trigger trg_validation_retiree after update or delete on personnage_parametres_modele
    for each row execute function rabattre_si_validation_retiree();

-- ---------------------------------------------------------------------------
-- 4. Ménage
-- ---------------------------------------------------------------------------
-- Ligne morte : le nom est normalisé sans accents avant comparaison, donc une entrée accentuée
-- ne peut jamais correspondre.
delete from ref_termes_interdits where terme = 'lycéenne';

-- Index qui documentaient une intention que le code n'a pas. Mesuré : zéro balayage sur les deux,
-- et le second est doublé par l'index de la contrainte `unique (personnage_id, version)`, que le
-- planificateur retient systématiquement.
drop index idx_termes_actifs;
drop index idx_historique_personnage_version;
