-- Une modification de compagnon sans version inscrite devient impossible.
--
-- # Ce que la phase 1.4 exige, et ce qui la tenait jusqu'ici
--
-- « Toute écriture sur les tables `personnage_*` s'accompagne d'une ligne dans
-- `personnage_historique_versions`, dans la même transaction. »
--
-- C'était une phrase dans un document, tenue par deux appelants qui y pensaient. Rien n'obligeait
-- le troisième. Et la phase 1.5 — l'onboarding depuis Telegram — va précisément multiplier les
-- écrivains : chaque étape du parcours écrit des traits.
--
-- C'est le motif que ce dépôt corrige à chaque revue : une garantie énoncée, tenue nulle part.
-- La migration 0006 l'a fait pour le texte éditorial, la 0007 pour le registre des coûts.
--
-- # Pourquoi un trigger de contrainte différé, et pas un trigger ordinaire
--
-- Un trigger ordinaire se déclenche à l'instruction, donc **avant** que la version ait pu être
-- inscrite : une transaction légitime — écrire l'apparence, les traits, les curseurs, puis la
-- version — échouerait à sa première ligne.
--
-- Un `constraint trigger ... deferrable initially deferred` se déclenche au `commit`. À ce
-- moment-là, l'ordre des écritures à l'intérieur de la transaction n'a plus d'importance : seul
-- compte le fait qu'une version existe. C'est exactement la propriété que la phase 1.4 demande.
--
-- # Ce qui n'est pas couvert, et pourquoi
--
-- # Une conséquence à connaître : le renommage aussi
--
-- `personnages` n'est pas une table `personnage_*`, donc renommer un compagnon n'est pas visé
-- directement. Mais la migration 0006 fait de tout renommage une **révocation**, qui écrit
-- `personnage_parametres_modele` — et c'est ce write-là que la contrainte voit. Un renommage
-- exige donc une version, par conséquence plutôt que par désignation.
--
-- C'est le bon résultat, et il vaut d'être dit : le nom est le seul texte libre du compagnon,
-- et 0006 le décrit comme « le second chemin par lequel du texte non modéré atteignait le
-- prompt ». Un changement de nom mérite d'être raconté autant qu'un changement de trait.
--
-- # Ce qui n'est pas couvert, et pourquoi
--
-- Le `delete`. La seule suppression légitime est la purge RGPD, qui efface aussi l'historique :
-- lui demander d'y inscrire une version serait lui demander d'écrire dans ce qu'elle détruit.
-- Les écritures de traits, elles, ne suppriment jamais — elles insèrent ou remplacent.
create or replace function exiger_une_version() returns trigger as $$
declare
    cible uuid := new.personnage_id;
begin
    -- `transaction_timestamp()` est l'instant d'ouverture de la transaction, et `modifie_le`
    -- vaut `now()`, qui est le même instant pour toute la transaction. La comparaison isole
    -- donc exactement les versions inscrites ici, sans avoir à passer d'identifiant.
    if not exists (
        select 1 from personnage_historique_versions
         where personnage_id = cible and modifie_le >= transaction_timestamp()
    ) then
        raise exception
            'personnage % : toute modification doit inscrire une version dans la même transaction',
            cible
            using errcode = 'restrict_violation',
                  hint = 'appeler personnage::inscrire_version avant de committer';
    end if;
    return null;
end;
$$ language plpgsql;

create constraint trigger trg_apparence_versionnee
    after insert or update on personnage_apparence
    deferrable initially deferred for each row execute function exiger_une_version();
create constraint trigger trg_archetypes_versionnee
    after insert or update on personnage_archetypes
    deferrable initially deferred for each row execute function exiger_une_version();
create constraint trigger trg_tons_versionnee
    after insert or update on personnage_tons
    deferrable initially deferred for each row execute function exiger_une_version();
create constraint trigger trg_gradues_versionnee
    after insert or update on personnage_parametres_gradues
    deferrable initially deferred for each row execute function exiger_une_version();
create constraint trigger trg_interaction_versionnee
    after insert or update on personnage_parametres_interaction
    deferrable initially deferred for each row execute function exiger_une_version();
create constraint trigger trg_modele_versionnee
    after insert or update on personnage_parametres_modele
    deferrable initially deferred for each row execute function exiger_une_version();
