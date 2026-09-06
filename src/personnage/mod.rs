//! Le compagnon : ses traits, et le prompt qu'ils composent.
//!
//! # La règle qui gouverne tout ce module
//!
//! > L'utilisateur choisit des options dans des listes contrôlées. Le service compose le prompt.
//! > L'utilisateur ne tape jamais le prompt système lui-même.
//!
//! Aucune fonction ici n'accepte de texte libre destiné au modèle. Le seul champ que
//! l'utilisateur écrit est le **nom**, et c'est pour cela qu'il est la seule chose que la
//! modération ait réellement à examiner : tout le reste vient de descriptions écrites une fois,
//! au catalogue.
//!
//! # Pourquoi la composition est une fonction pure
//!
//! [`composer`] ne touche pas la base : elle prend des [`Traits`] déjà chargés et rend un
//! [`Prompt`]. La lecture vit dans [`charger`]. Cette séparation n'est pas une élégance — c'est
//! ce qui permet d'éprouver la résolution des fusions et l'application des plafonds sur des cas
//! qu'il serait pénible de fabriquer en base, et de **lire** le prompt produit dans la sortie
//! des tests. C'est le texte qui compte, pas le fait que la fonction rende `Ok`.

pub mod moderation;
pub mod sceau;
pub mod regles;

use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::ErreurBase;
use crate::db::catalogues::{self, Fusion};

/// Un trait retenu par l'utilisateur : ce que le catalogue en dit.
#[derive(Debug, Clone)]
pub struct TraitRetenu {
    /// Code du catalogue.
    pub code: String,
    /// Libellé affiché.
    pub libelle: String,
    /// Description éditoriale, reprise dans le prompt.
    pub description: String,
}

/// Une composition : un trait principal et jusqu'à deux secondaires.
#[derive(Debug, Clone)]
pub struct Composition {
    /// Le trait dominant.
    pub principal: TraitRetenu,
    /// Les nuances, dans l'ordre choisi.
    pub secondaires: Vec<TraitRetenu>,
}

/// L'apparence, en libellés déjà résolus depuis les catalogues.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct Apparence {
    /// Genre.
    pub genre: String,
    /// Âge apparent **plancher**, en années.
    ///
    /// Un nombre et non le libellé de la tranche, et c'est une correction. Le libellé était ce
    /// qui atteignait le modèle, et rien ne le contraignait : `check (age_min >= 25)` gardait
    /// une colonne que la composition ne lisait pas. Une seule écriture —
    /// `update ref_tranches_age_apparent set libelle = 'Adolescente de 16 ans'` — passait la
    /// contrainte, passait les tests, passait la modération, et le prompt disait « Adolescente
    /// de 16 ans ».
    ///
    /// Composer depuis le nombre fait descendre la garantie jusqu'à ce que le modèle lit.
    /// `libelle` redevient ce qu'il aurait dû rester : un texte d'interface.
    pub age_min: i16,
    /// Morphologie.
    pub morphologie: String,
    /// Couleur de cheveux, si choisie.
    pub couleur_cheveux: Option<String>,
    /// Longueur de cheveux, si choisie.
    pub longueur_cheveux: Option<String>,
    /// Couleur des yeux, si choisie.
    pub couleur_yeux: Option<String>,
    /// Style vestimentaire, si choisi.
    pub style_vestimentaire: Option<String>,
}

/// Un curseur, avec sa valeur **déjà plafonnée** par la juridiction.
#[derive(Debug, Clone)]
pub struct CurseurEffectif {
    /// Code du paramètre.
    pub code: String,
    /// Libellé affiché.
    pub libelle: String,
    /// Valeur après application du plafond.
    pub valeur: Decimal,
    /// Valeur choisie avant plafonnement, quand elle en diffère.
    pub avant_plafond: Option<Decimal>,
}

/// Tout ce qu'il faut pour composer, déjà lu et résolu.
#[derive(Debug, Clone)]
pub struct Traits {
    /// Le nom donné par l'utilisateur — le seul texte libre du compagnon.
    pub nom: String,
    /// L'apparence.
    pub apparence: Apparence,
    /// Les archétypes, avec leur fusion si le couple est répertorié.
    pub archetypes: Composition,
    /// La fusion d'archétypes reconnue pour le premier secondaire, s'il en existe une.
    pub fusion_archetypes: Option<Fusion>,
    /// Les tons.
    pub tons: Composition,
    /// La fusion de tons reconnue, s'il en existe une.
    pub fusion_tons: Option<Fusion>,
    /// Les curseurs de personnalité, déjà plafonnés.
    pub curseurs: Vec<CurseurEffectif>,
    /// `courte`, `moyenne` ou `longue`.
    pub longueur_reponse: String,
}

/// Un prompt composé.
///
/// Ne porte plus son empreinte : sceller demande une **clé**, qui vit dans l'environnement du
/// processus et non dans les traits. Composer reste une fonction pure de ce que l'utilisateur a
/// choisi ; apposer le sceau est un second geste, et c'est [`sceau::Sceau`] qui en répond.
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Le texte envoyé au modèle.
    pub texte: String,
}

/// Les cinq paliers par lesquels un curseur devient une phrase.
///
/// # Pourquoi des paliers et pas le nombre
///
/// Écrire « humour : 0,63 » dans un prompt demande au modèle d'interpréter une échelle qu'il ne
/// connaît pas, et deux valeurs voisines produisent des réponses arbitrairement différentes.
/// Cinq paliers nommés donnent une consigne que le modèle sait suivre, et rendent la
/// composition **stable** : un curseur qui glisse de 0,61 à 0,64 ne change pas le prompt, donc
/// ne redemande pas de modération.
const PALIERS: [(&str, i64); 5] = [
    ("très peu", 20),
    ("peu", 40),
    ("modérément", 60),
    ("beaucoup", 80),
    ("énormément", 100),
];

/// Traduit le code de longueur de cheveux en français lisible.
///
/// `longueur_cheveux` est contraint par un `check` et non par une table de référence — il n'a
/// donc pas de libellé en base. Sans cette traduction, `mi_longs` partait tel quel dans le
/// prompt : un code interne livré à un modèle, qui l'aurait lu comme un mot inconnu.
const fn longueur_lisible(code: &str) -> &str {
    match code.as_bytes() {
        b"courts" => "courts",
        b"mi_longs" => "mi-longs",
        b"longs" => "longs",
        // La contrainte de base rend ce cas inatteignable ; le traiter évite d'avoir à le
        // supposer.
        _ => "de longueur indéterminée",
    }
}

/// Traduit un curseur en palier.
fn palier(valeur: Decimal) -> &'static str {
    let centiemes = (valeur * Decimal::ONE_HUNDRED)
        .round()
        .try_into()
        .unwrap_or(0_i64);
    PALIERS
        .iter()
        .find(|(_, plafond)| centiemes <= *plafond)
        .map_or("énormément", |(nom, _)| *nom)
}

/// Compose le prompt système à partir de traits déjà résolus.
///
/// # Ordre de résolution
///
/// Celui du document de schéma, et il n'est pas indifférent : ce qui vient en dernier pèse le
/// plus pour un modèle.
///
/// 1. identité — nom et apparence ;
/// 2. personnalité — fusions d'archétypes puis de tons ;
/// 3. curseurs, déjà plafonnés par la juridiction ;
/// 4. registre — longueur de réponse ;
/// 5. **règles fixes**, toujours en dernier et non paramétrables.
#[must_use]
pub fn composer(traits: &Traits) -> Prompt {
    let mut t = String::with_capacity(2048);

    // 1. Identité
    t.push_str(&format!(
        "Tu es {}. Tu es un compagnon de conversation, et tu restes ce personnage tout au long \
         de l'échange.\n\n",
        traits.nom
    ));
    t.push_str("Apparence :\n");
    t.push_str(&format!(
        "- {}, apparence d'au moins {} ans\n",
        traits.apparence.genre, traits.apparence.age_min
    ));
    t.push_str(&format!("- silhouette {}\n", traits.apparence.morphologie));
    if let Some(couleur) = &traits.apparence.couleur_cheveux {
        let longueur = traits
            .apparence
            .longueur_cheveux
            .as_deref()
            .map_or_else(String::new, |code| format!(" {}", longueur_lisible(code)));
        t.push_str(&format!("- cheveux {couleur}{longueur}\n"));
    }
    if let Some(yeux) = &traits.apparence.couleur_yeux {
        t.push_str(&format!("- yeux {yeux}\n"));
    }
    if let Some(style) = &traits.apparence.style_vestimentaire {
        t.push_str(&format!("- style {style}\n"));
    }

    // 2. Personnalité
    t.push_str("\nPersonnalité :\n");
    t.push_str(&decrire(
        &traits.archetypes,
        traits.fusion_archetypes.as_ref(),
    ));
    t.push_str("\nFaçon de parler :\n");
    t.push_str(&decrire(&traits.tons, traits.fusion_tons.as_ref()));

    // 3. Curseurs
    if !traits.curseurs.is_empty() {
        t.push_str("\nDosage :\n");
        for curseur in &traits.curseurs {
            t.push_str(&format!(
                "- {} : {}\n",
                curseur.libelle.to_lowercase(),
                palier(curseur.valeur)
            ));
        }
    }

    // 4. Registre
    t.push_str(&format!(
        "\nTes réponses sont de longueur {}.\n\n",
        traits.longueur_reponse
    ));

    // 5. Règles fixes, en dernier
    t.push_str(&regles::bloc());

    Prompt { texte: t }
}


/// Décrit une composition, en préférant la fusion quand le couple est répertorié.
///
/// # La résolution, telle que le document la définit
///
/// 1. si une fusion existe pour (principal, premier secondaire), sa description **remplace**
///    l'addition des deux ;
/// 2. sinon, description du principal puis de chaque secondaire, de façon additive ;
/// 3. s'il y a deux secondaires et qu'une seule fusion est reconnue, la fusion couvre la
///    première paire et la description simple du second secondaire s'ajoute par-dessus.
fn decrire(composition: &Composition, fusion: Option<&Fusion>) -> String {
    let mut t = String::new();
    match fusion {
        Some(fusion) => {
            t.push_str(&format!(
                "- {} : {}\n",
                fusion.nom_fusion, fusion.description_fusion
            ));
            // La fusion a consommé le principal et le PREMIER secondaire ; le second, s'il
            // existe, s'ajoute simplement.
            for secondaire in composition.secondaires.iter().skip(1) {
                t.push_str(&format!(
                    "- {} : {}\n",
                    secondaire.libelle, secondaire.description
                ));
            }
        }
        None => {
            t.push_str(&format!(
                "- {} : {}\n",
                composition.principal.libelle, composition.principal.description
            ));
            for secondaire in &composition.secondaires {
                t.push_str(&format!(
                    "- {} : {}\n",
                    secondaire.libelle, secondaire.description
                ));
            }
        }
    }
    t
}

/// Charge les traits d'un compagnon depuis la base, fusions et plafonds résolus.
///
/// Le `code_pays` est celui **déclaré** par l'utilisateur : c'est lui qui détermine les
/// plafonds. Une absence de plafond pour ce pays ne plafonne rien, ce qui est le bon défaut
/// pour un pays qu'on n'a pas encore examiné — à condition de ne pas y ouvrir le service.
///
/// # Errors
///
/// [`ErreurBase`] si une lecture échoue, ou si le compagnon est incomplet — sans archétype
/// principal, par exemple, il n'y a rien à composer.
pub async fn charger(
    pool: &PgPool,
    personnage_id: Uuid,
    code_pays: Option<&str>,
) -> Result<Traits, ErreurBase> {
    let (nom,): (String,) = sqlx::query_as("select nom from personnages where id = $1")
        .bind(personnage_id)
        .fetch_one(pool)
        .await?;

    let apparence = charger_apparence(pool, personnage_id).await?;
    let archetypes = charger_composition(pool, personnage_id, Cible::Archetypes).await?;
    let tons = charger_composition(pool, personnage_id, Cible::Tons).await?;

    // La fusion se cherche sur le PREMIER secondaire seulement : c'est lui qui nuance le
    // principal, le second n'est qu'une couleur de plus.
    let fusion_archetypes = match archetypes.secondaires.first() {
        Some(second) => {
            catalogues::fusion_archetypes(pool, &archetypes.principal.code, &second.code).await?
        }
        None => None,
    };
    let fusion_tons = match tons.secondaires.first() {
        Some(second) => catalogues::fusion_tons(pool, &tons.principal.code, &second.code).await?,
        None => None,
    };

    let curseurs = charger_curseurs(pool, personnage_id, code_pays).await?;

    let longueur_reponse: String = sqlx::query_scalar(
        "select longueur_reponse from personnage_parametres_interaction where personnage_id = $1",
    )
    .bind(personnage_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| "moyenne".to_owned());

    Ok(Traits {
        nom,
        apparence,
        archetypes,
        fusion_archetypes,
        tons,
        fusion_tons,
        curseurs,
        longueur_reponse,
    })
}

/// Laquelle des deux compositions — archétypes ou tons — est visée.
///
/// Les deux partagent exactement la même forme : une table de liaison, une table de référence,
/// une colonne. Les faire voyager séparément demandait trois arguments à chaque fonction qui
/// les touche, et rien n'empêchait de les mélanger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cible {
    /// Les archétypes : ce que le compagnon **est**.
    Archetypes,
    /// Les tons : la façon dont il **parle**.
    Tons,
}

impl Cible {
    /// La table de liaison, la table de référence, et la colonne qui les relie.
    ///
    /// Interpolées dans les requêtes, ce qui n'est sûr que parce qu'elles viennent d'un énuméré
    /// fermé — aucune chaîne extérieure ne les atteint.
    #[must_use]
    pub const fn tables(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Archetypes => ("personnage_archetypes", "ref_archetypes", "archetype_id"),
            Self::Tons => ("personnage_tons", "ref_tons", "ton_id"),
        }
    }

    /// Le préfixe des arguments de ligne de commande — `archetype`, `archetype2`…
    #[must_use]
    pub const fn prefixe(self) -> &'static str {
        match self {
            Self::Archetypes => "archetype",
            Self::Tons => "ton",
        }
    }
}

async fn charger_composition(
    pool: &PgPool,
    personnage_id: Uuid,
    cible: Cible,
) -> Result<Composition, ErreurBase> {
    let (liaison, reference, colonne) = cible.tables();
    let lignes: Vec<(String, String, String, String, Option<i16>)> = sqlx::query_as(&format!(
        "select r.code, r.libelle, r.description, l.role, l.rang
           from {liaison} l join {reference} r on r.id = l.{colonne}
          where l.personnage_id = $1
          order by case l.role when 'principal' then 0 else 1 end, l.rang"
    ))
    .bind(personnage_id)
    .fetch_all(pool)
    .await?;

    let mut principal = None;
    let mut secondaires = Vec::new();
    for (code, libelle, description, role, _) in lignes {
        let trait_ = TraitRetenu {
            code,
            libelle,
            description,
        };
        if role == "principal" {
            principal = Some(trait_);
        } else {
            secondaires.push(trait_);
        }
    }

    // Un compagnon sans principal ne se compose pas. L'index unique garantit qu'il n'y en a
    // jamais deux ; rien ne garantit qu'il y en ait un, et c'est ici qu'on s'en aperçoit.
    let principal = principal.ok_or_else(|| ErreurBase::Requete(sqlx::Error::RowNotFound))?;
    Ok(Composition {
        principal,
        secondaires,
    })
}

async fn charger_apparence(pool: &PgPool, personnage_id: Uuid) -> Result<Apparence, ErreurBase> {
    // Les colonnes sont nommées comme les champs, et `FromRow` fait le reste : un 7-uplet
    // positionnel se serait décalé en silence le jour où une colonne s'insère au milieu.
    sqlx::query_as::<_, Apparence>(
        "select g.libelle    as genre,
                t.age_min,
                m.libelle    as morphologie,
                ch.libelle   as couleur_cheveux,
                a.longueur_cheveux,
                y.libelle    as couleur_yeux,
                s.libelle    as style_vestimentaire
           from personnage_apparence a
           join ref_genres g on g.id = a.genre_id
           join ref_tranches_age_apparent t on t.id = a.tranche_age_id
           join ref_morphologies m on m.id = a.morphologie_id
           left join ref_couleurs_cheveux ch on ch.id = a.couleur_cheveux_id
           left join ref_couleurs_yeux y on y.id = a.couleur_yeux_id
           left join ref_styles_vestimentaires s on s.id = a.style_vestimentaire_id
          where a.personnage_id = $1",
    )
    .bind(personnage_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ErreurBase::Requete(sqlx::Error::RowNotFound))
}

async fn charger_curseurs(
    pool: &PgPool,
    personnage_id: Uuid,
    code_pays: Option<&str>,
) -> Result<Vec<CurseurEffectif>, ErreurBase> {
    // Deux corrections dans cette requête, et la seconde était un défaut réel.
    //
    // Le plafond est appliqué **dans la requête** et non après coup : c'est la même discipline
    // que pour les signaux commerciaux de la phase 2.6 — une vérification qu'une évolution du
    // code applicatif ne peut pas contourner par distraction.
    //
    // Et la jointure porte désormais sur `plafonnable_juridiction`, non sur le domaine. Le
    // filtre `domaine = 'personnalite'` servait de proxy à « ce curseur entre dans le prompt »,
    // or le seul paramètre marqué plafonnable — `intensite_suggestive` — est de domaine
    // `contenu` : les plafonds ne pouvaient s'appliquer qu'à des paramètres déclarés NON
    // plafonnables, et jamais à celui pour lequel le mécanisme légal a été construit. Le
    // drapeau, le commentaire du schéma et le code disaient trois choses différentes.
    let lignes: Vec<(String, String, Decimal, Option<Decimal>)> = sqlx::query_as(
        "select r.code, r.libelle, g.valeur, pl.valeur_max
           from personnage_parametres_gradues g
           join ref_parametres_gradues r on r.code = g.parametre_code
           left join ref_plafonds_juridiction pl
                  on pl.parametre_code = g.parametre_code
                 and pl.code_pays = $2
                 and r.plafonnable_juridiction
          where g.personnage_id = $1 and r.actif and r.entre_dans_le_prompt
          order by r.code",
    )
    .bind(personnage_id)
    .bind(code_pays)
    .fetch_all(pool)
    .await?;

    Ok(lignes
        .into_iter()
        .map(|(code, libelle, choisie, plafond)| {
            let valeur = plafond.map_or(choisie, |max| choisie.min(max));
            CurseurEffectif {
                code,
                libelle,
                valeur,
                avant_plafond: (valeur != choisie).then_some(choisie),
            }
        })
        .collect())
}

/// Soumet un compagnon à la modération, et inscrit ce qu'elle décide.
///
/// # Ce que cette fonction fait, et pourquoi d'un seul tenant
///
/// Elle compose, elle examine, elle écrit — dans **une seule transaction**. Séparer ces gestes
/// laisserait exister un instant où un prompt est écrit sans que la modération se soit
/// prononcée, et c'est précisément l'état que le verrou d'activation existe pour empêcher.
///
/// Accepté : le prompt et son empreinte sont écrits avec `valide_le`, et le compagnon devient
/// activable. Refusé : le statut passe à `rejete`, aucun prompt n'est écrit, et l'utilisateur
/// doit modifier ses choix.
///
/// Dans les deux cas une version est inscrite à l'historique : un refus fait partie de ce qu'on
/// doit pouvoir raconter.
///
/// # Errors
///
/// [`ErreurBase`] si une lecture ou une écriture échoue. Aucune écriture n'est conservée en cas
/// d'erreur — c'est l'objet de la transaction.
pub async fn valider(
    pool: &PgPool,
    personnage_id: Uuid,
    code_pays: Option<&str>,
    modele_cible: &str,
    sceau: &sceau::Sceau,
) -> Result<moderation::Verdict, ErreurBase> {
    let traits = charger(pool, personnage_id, code_pays).await?;
    let verdict = moderation::examiner_nom(pool, &traits.nom).await?;

    let mut tx = pool.begin().await?;

    let (statut, raison) = match &verdict {
        moderation::Verdict::Accepte => {
            let prompt = composer(&traits);
            let appose = sceau.apposer(&prompt.texte);
            sqlx::query(
                "insert into personnage_parametres_modele
                    (personnage_id, prompt_systeme_genere, prompt_systeme_sceau, modele_cible,
                     valide_le)
                 values ($1, $2, $3, $4, now())
                 on conflict (personnage_id) do update
                    set prompt_systeme_genere = excluded.prompt_systeme_genere,
                        prompt_systeme_sceau   = excluded.prompt_systeme_sceau,
                        modele_cible          = excluded.modele_cible,
                        version_prompt        = personnage_parametres_modele.version_prompt + 1,
                        valide_le             = now()",
            )
            .bind(personnage_id)
            .bind(&prompt.texte)
            .bind(&appose)
            .bind(modele_cible)
            .execute(&mut *tx)
            .await?;
            ("brouillon", "moderation_validation")
        }
        moderation::Verdict::Refuse(motif) => {
            // Le terme reconnu part au journal d'exploitation, jamais à l'utilisateur.
            tracing::warn!(
                compagnon = %personnage_id,
                motif = ?motif,
                "composition refusée par la modération"
            );
            // Un prompt existant est retiré : un compagnon rejeté ne doit rien conserver
            // d'activable, y compris ce qu'une validation précédente avait laissé.
            sqlx::query("delete from personnage_parametres_modele where personnage_id = $1")
                .bind(personnage_id)
                .execute(&mut *tx)
                .await?;
            ("rejete", "moderation_rejet")
        }
    };

    sqlx::query("update personnages set statut = $2 where id = $1")
        .bind(personnage_id)
        .bind(statut)
        .execute(&mut *tx)
        .await?;

    inscrire_version(&mut tx, personnage_id, raison).await?;
    tx.commit().await?;
    Ok(verdict)
}

/// Active un compagnon validé.
///
/// **Seul écrivain de `statut = 'actif'` dans tout le crate.** Il n'en existait aucun : la
/// validation laissait le compagnon en `brouillon`, la ligne de commande annonçait
/// « activable », et le seul chemin vers l'état actif était un `psql`. Le verrou construit pour
/// protéger ce geste gardait donc une porte que le produit ne savait pas ouvrir.
///
/// L'activation reste **délibérée** et séparée de la validation : passer la modération autorise
/// à parler, cela ne veut pas dire qu'on le veut tout de suite. Le déclencheur en base reste le
/// filet — cette fonction ne le remplace pas, elle lui donne un appelant légitime.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si le prompt n'est pas validé : le déclencheur refuse, et le message
/// de PostgreSQL nomme le compagnon.
pub async fn activer(pool: &PgPool, personnage_id: Uuid) -> Result<(), ErreurBase> {
    sqlx::query("update personnages set statut = 'actif' where id = $1 and supprime_le is null")
        .bind(personnage_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Ce qu'une vérification d'intégrité a constaté.
#[derive(Debug, PartialEq, Eq)]
pub enum Integrite {
    /// Le prompt stocké correspond à son empreinte **et** à ce que les traits composent.
    Intacte,
    /// Le texte stocké ne correspond plus à son empreinte : la ligne a été altérée.
    TexteAltere,
    /// Le texte est intact, mais les traits ou le catalogue ont changé depuis la validation.
    ///
    /// C'est le cas utile — celui qu'aucune contrainte ne peut attraper, parce que le prompt
    /// validé reste parfaitement valide en lui-même. Il ne décrit simplement plus le compagnon.
    DeriveDepuisValidation,
    /// Aucun prompt validé : il n'y a rien à vérifier.
    PasDePromptValide,
}

/// Vérifie que le prompt validé dit encore ce qu'il disait.
///
/// # Pourquoi cette fonction devait exister
///
/// `prompt_systeme_sceau` était écrit et **jamais relu**. Une empreinte que personne ne compare
/// n'est pas une garantie ; et comparée à elle seule, elle n'aurait rien attrapé d'utile — elle
/// vit dans la même ligne que le texte qu'elle atteste, donc la console qui modifie l'un modifie
/// l'autre. C'est un contrôle de cohérence, pas un sceau.
///
/// La comparaison qui a de la valeur est la seconde : recomposer depuis les traits actuels et
/// constater que le résultat diffère. Elle attrape ce qu'aucune contrainte ne peut voir — une
/// description de catalogue modifiée, un trait changé, un plafond de juridiction posé après coup.
///
/// # Errors
///
/// [`ErreurBase`] si une lecture échoue.
pub async fn verifier_integrite(
    pool: &PgPool,
    personnage_id: Uuid,
    code_pays: Option<&str>,
    sceau: &sceau::Sceau,
) -> Result<Integrite, ErreurBase> {
    let stocke: Option<(String, String)> = sqlx::query_as(
        "select prompt_systeme_genere, prompt_systeme_sceau
           from personnage_parametres_modele
          where personnage_id = $1 and valide_le is not null",
    )
    .bind(personnage_id)
    .fetch_optional(pool)
    .await?;

    let Some((texte, empreinte)) = stocke else {
        return Ok(Integrite::PasDePromptValide);
    };

    if !sceau.verifier(&texte, &empreinte) {
        return Ok(Integrite::TexteAltere);
    }

    let recompose = composer(&charger(pool, personnage_id, code_pays).await?);
    Ok(if recompose.texte == texte {
        Integrite::Intacte
    } else {
        Integrite::DeriveDepuisValidation
    })
}

/// Inscrit une version à l'historique, avec l'instantané complet du compagnon.
///
/// Publique parce que la création doit l'appeler : la spécification dit que toute écriture dans
/// une table `personnage_*` s'accompagne d'une version, et la création n'en écrivait aucune —
/// `'creation'` figurait dans la contrainte et n'était jamais produite.
///
/// # Errors
///
/// [`ErreurBase`] si l'écriture échoue.
///
/// L'instantané est construit **par la base**, en une requête : le reconstituer en Rust
/// demanderait de relire chaque table et de n'en oublier aucune — or c'est exactement ce qu'un
/// historique existe pour rendre inutile.
pub async fn inscrire_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    personnage_id: Uuid,
    raison: &str,
) -> Result<(), ErreurBase> {
    sqlx::query(
        "insert into personnage_historique_versions
            (personnage_id, version, modifie_par, raison, etat_complet)
         select p.id,
                coalesce((select max(version) + 1 from personnage_historique_versions
                           where personnage_id = p.id), 1),
                p.utilisateur_id,
                $2,
                jsonb_build_object(
                    'personnage', to_jsonb(p.*),
                    'apparence',  (select to_jsonb(a.*) from personnage_apparence a
                                    where a.personnage_id = p.id),
                    'archetypes', (select jsonb_agg(to_jsonb(x.*)) from personnage_archetypes x
                                    where x.personnage_id = p.id),
                    'tons',       (select jsonb_agg(to_jsonb(t.*)) from personnage_tons t
                                    where t.personnage_id = p.id),
                    'curseurs',   (select jsonb_agg(to_jsonb(g.*))
                                     from personnage_parametres_gradues g
                                    where g.personnage_id = p.id),
                    'interaction',(select to_jsonb(i.*) from personnage_parametres_interaction i
                                    where i.personnage_id = p.id),
                    'modele',     (select to_jsonb(m.*) from personnage_parametres_modele m
                                    where m.personnage_id = p.id))
           from personnages p where p.id = $1",
    )
    .bind(personnage_id)
    .bind(raison)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn trait_de(code: &str, libelle: &str, description: &str) -> TraitRetenu {
        TraitRetenu {
            code: code.to_owned(),
            libelle: libelle.to_owned(),
            description: description.to_owned(),
        }
    }

    fn curseur(code: &str, libelle: &str, centiemes: i64) -> CurseurEffectif {
        CurseurEffectif {
            code: code.to_owned(),
            libelle: libelle.to_owned(),
            valeur: Decimal::new(centiemes, 2),
            avant_plafond: None,
        }
    }

    fn lea() -> Traits {
        Traits {
            nom: "Léa".to_owned(),
            apparence: Apparence {
                genre: "Femme".to_owned(),
                age_min: 25,
                morphologie: "Élancée".to_owned(),
                couleur_cheveux: Some("Bruns".to_owned()),
                longueur_cheveux: Some("mi_longs".to_owned()),
                couleur_yeux: Some("Verts".to_owned()),
                style_vestimentaire: Some("Décontracté".to_owned()),
            },
            archetypes: Composition {
                principal: trait_de(
                    "timide",
                    "Timide",
                    "réservé au premier abord, se livre peu à peu",
                ),
                secondaires: vec![trait_de(
                    "dominant",
                    "Dominant",
                    "assuré, mène la conversation",
                )],
            },
            fusion_archetypes: None,
            tons: Composition {
                principal: trait_de("tendre", "Tendre", "doux, enveloppant"),
                secondaires: vec![],
            },
            fusion_tons: None,
            curseurs: vec![
                curseur("humour", "Humour", 70),
                curseur("affection", "Affection", 85),
                curseur("assurance", "Assurance", 30),
            ],
            longueur_reponse: "moyenne".to_owned(),
        }
    }

    #[test]
    fn le_prompt_compose_se_lit() {
        // Ce test n'a pas d'assertion intéressante : il existe pour que le PROMPT soit lisible
        // dans la sortie. C'est le texte qui part au modèle — c'est lui qu'un humain doit
        // relire, pas le fait que la fonction rende `Ok`.
        let prompt = composer(&lea());
        println!("=============== PROMPT COMPOSÉ ===============");
        println!("{}", prompt.texte);
        println!("==============================================");
        assert!(prompt.texte.contains("Léa"));
    }

    #[test]
    fn les_regles_fixes_viennent_toujours_en_dernier() {
        // L'ordre n'est pas cosmétique : un modèle accorde plus de poids à ce qui vient en
        // dernier, et aucune valeur de paramètre ne doit pouvoir contredire ces règles.
        let prompt = composer(&lea());
        let position_regles = prompt
            .texte
            .find("Règles absolues")
            .expect("les règles y sont");
        let position_dosage = prompt.texte.find("Dosage").expect("les curseurs y sont");
        let position_identite = prompt.texte.find("Tu es Léa").expect("l'identité y est");
        println!(
            "identité en {position_identite}, dosage en {position_dosage}, règles en {position_regles}"
        );
        assert!(position_identite < position_dosage);
        assert!(
            position_dosage < position_regles,
            "les règles doivent venir après tout le reste"
        );

        // Et rien ne suit les règles.
        let apres = &prompt.texte[position_regles..];
        println!("--- ce qui suit « Règles absolues » ---\n{apres}");
        assert!(
            apres.contains("mineur"),
            "l'interdit majeur doit être dans le bloc final"
        );
    }

    #[test]
    fn une_fusion_remplace_l_addition_des_deux_descriptions() {
        let mut traits = lea();
        let sans = composer(&traits);
        println!("--- SANS fusion ---");
        println!("{}", extraire_personnalite(&sans.texte));

        traits.fusion_archetypes = Some(Fusion {
            nom_fusion: "Yandere".to_owned(),
            description_fusion: "réservé en surface et étonnamment affirmé".to_owned(),
        });
        let avec = composer(&traits);
        println!("--- AVEC fusion ---");
        println!("{}", extraire_personnalite(&avec.texte));

        assert!(sans.texte.contains("Timide") && sans.texte.contains("Dominant"));
        assert!(avec.texte.contains("Yandere"));
        assert!(
            !avec.texte.contains("réservé au premier abord"),
            "la fusion doit REMPLACER l'addition, pas s'y ajouter"
        );
    }

    #[test]
    fn avec_deux_secondaires_la_fusion_couvre_le_premier_et_le_second_s_ajoute() {
        // Le cas que le document décrit explicitement, et le seul où la résolution est
        // ambiguë si on ne l'a pas écrite : la fusion consomme le principal et le PREMIER
        // secondaire, le second reste une couleur de plus.
        let mut traits = lea();
        traits
            .archetypes
            .secondaires
            .push(trait_de("joueur", "Joueur", "taquin, aime les piques"));
        traits.fusion_archetypes = Some(Fusion {
            nom_fusion: "Yandere".to_owned(),
            description_fusion: "réservé en surface et étonnamment affirmé".to_owned(),
        });
        let prompt = composer(&traits);
        println!("{}", extraire_personnalite(&prompt.texte));

        assert!(
            prompt.texte.contains("Yandere"),
            "la fusion couvre principal + secondaire 1"
        );
        assert!(
            prompt.texte.contains("Joueur"),
            "le second secondaire doit s'ajouter"
        );
        assert!(
            !prompt.texte.contains("mène la conversation"),
            "le premier secondaire est consommé par la fusion"
        );
    }

    #[test]
    fn un_curseur_devient_un_palier_et_reste_stable_entre_deux_valeurs_voisines() {
        // Des paliers plutôt que le nombre : « humour : 0,63 » demande au modèle d'interpréter
        // une échelle qu'il ne connaît pas. Et la stabilité compte pour une raison concrète —
        // un curseur qui glisse sans changer le prompt ne redemande pas de modération.
        for centiemes in [0, 10, 20, 21, 40, 50, 61, 64, 80, 95, 100] {
            let valeur = Decimal::new(centiemes, 2);
            println!("{valeur:>5} -> {}", palier(valeur));
        }
        assert_eq!(palier(Decimal::new(61, 2)), palier(Decimal::new(64, 2)));
        assert_eq!(palier(Decimal::ZERO), "très peu");
        assert_eq!(palier(Decimal::ONE), "énormément");
        assert_ne!(palier(Decimal::new(20, 2)), palier(Decimal::new(21, 2)));
    }

    #[test]
    fn le_prompt_change_avec_les_traits_et_pas_autrement() {
        // La composition doit être strictement déterminée par les traits : c'est ce qui permet
        // à `verifier_integrite` de recomposer et de constater une dérive. La comparaison porte
        // sur le TEXTE, et non sur son sceau — celui-ci demande une clé, et comparer des
        // scellés reviendrait à éprouver le HMAC plutôt que la composition.
        let a = composer(&lea());
        let b = composer(&lea());
        assert_eq!(a.texte, b.texte, "la composition doit être déterministe");

        let mut autre = lea();
        autre.curseurs[0].valeur = Decimal::new(10, 2);
        let c = composer(&autre);
        println!("humour 0,70 -> {}", ligne_de_dosage(&a.texte, "humour"));
        println!("humour 0,10 -> {}", ligne_de_dosage(&c.texte, "humour"));
        assert_ne!(
            a.texte, c.texte,
            "un curseur qui change de palier change le prompt"
        );

        let mut voisin = lea();
        voisin.curseurs[0].valeur = Decimal::new(75, 2);
        let texte_voisin = composer(&voisin).texte;
        println!(
            "humour 0,75 -> {} (même palier que 0,70)",
            ligne_de_dosage(&texte_voisin, "humour")
        );
        assert_eq!(
            a.texte, texte_voisin,
            "à l'intérieur d'un palier, le prompt ne bouge pas"
        );
    }

    /// La ligne de dosage d'un curseur, pour que les sorties de test montrent ce qui change.
    fn ligne_de_dosage(texte: &str, code: &str) -> String {
        texte
            .lines()
            .find(|ligne| ligne.contains(code))
            .unwrap_or("(absente)")
            .trim()
            .to_owned()
    }

    /// Extrait la section personnalité, pour que les sorties de test restent lisibles.
    fn extraire_personnalite(texte: &str) -> String {
        let debut = texte.find("Personnalité :").unwrap_or(0);
        let fin = texte.find("Dosage").unwrap_or(texte.len());
        texte[debut..fin].trim().to_owned()
    }
}
