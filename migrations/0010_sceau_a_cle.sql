-- Le sceau du prompt devient un HMAC : tous les sceaux existants cessent d'être valides.
--
-- # Ce que la migration doit faire, et ce qu'elle ne peut pas faire
--
-- `prompt_systeme_hash` contenait un `sha256` du texte, calculable par quiconque avait la ligne.
-- Il contient désormais un HMAC-SHA256 dont la clé vit dans l'environnement du processus. Une
-- migration n'a pas cette clé — c'est précisément l'intérêt — donc elle ne peut pas resceller.
--
-- La seule conduite honnête est donc de **révoquer** : chaque compagnon repasse par la
-- modération, qui apposera un vrai sceau. Rien n'est perdu (traits, apparence, curseurs,
-- historique demeurent) ; seul le verrou d'activation se referme jusqu'à revalidation.
--
-- C'est le coût de la bascule, et c'est la raison pour laquelle elle est faite maintenant :
-- aujourd'hui la table ne contient que des compagnons d'essai. Elle contiendra des compagnons
-- de vraies personnes, et la même migration demanderait alors une campagne de revalidation.

-- `valide_le` à nul suffit : le trigger `trg_validation_retiree` de la migration 0006 rabat les
-- compagnons actifs en `brouillon`, ce qui est exactement l'invariant « actif ⇒ prompt validé ».
-- Le trigger de 0008 ne se déclenche pas — le texte du prompt ne change pas, seule sa validation.
update personnage_parametres_modele set valide_le = null where valide_le is not null;

-- L'ancien nom disait ce que la colonne contenait ; il ne le dit plus. Un « hash » se recalcule
-- avec ce qu'on a sous la main, un « sceau » demande une clé — et la confusion entre les deux
-- est exactement ce qui a laissé le contrôle passer pour une garantie.
alter table personnage_parametres_modele rename column prompt_systeme_hash to prompt_systeme_sceau;

comment on column personnage_parametres_modele.prompt_systeme_sceau is
    'HMAC-SHA256 du prompt, clé dans l''environnement du processus. La base ne contient pas de '
    'quoi en forger un : c''est ce qui distingue un sceau d''un contrôle de cohérence.';

-- Le trigger de révocation de la migration 0008 nomme l'ancienne colonne. Le corps d'une
-- fonction plpgsql est résolu à l'**exécution** : le renommage ci-dessus ne l'aurait pas
-- signalé, et la révocation aurait cessé de fonctionner au premier `update` — silencieusement,
-- ce qui est la pire façon pour une garantie de disparaître.
create or replace function revoquer_sur_changement_de_prompt() returns trigger as $$
begin
    if (new.prompt_systeme_genere is distinct from old.prompt_systeme_genere
        or new.prompt_systeme_sceau is distinct from old.prompt_systeme_sceau)
       and new.valide_le is not distinct from old.valide_le
    then
        new.valide_le := null;
    end if;
    return new;
end;
$$ language plpgsql;
