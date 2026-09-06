//! Ce dont le worker a besoin pour tenir une conversation : à qui il parle, et quoi inscrire.
//!
//! # Pourquoi le prompt est **lu** et non recomposé
//!
//! [`crate::personnage::composer`] sait reconstruire le prompt système à partir des traits. Le
//! worker ne l'appelle pas, pour deux raisons dont la seconde est la vraie :
//!
//! 1. c'est huit lectures de tables contre une, à chaque message ;
//! 2. c'est le texte stocké que la **modération a approuvé**. Une recomposition pourrait en
//!    diverger — une description de catalogue modifiée, un plafond de juridiction posé après
//!    coup — et le compagnon parlerait alors avec un prompt que personne n'a validé.
//!
//! # Ce qui remplace la recomposition
//!
//! Une vérification d'empreinte, faite ici à chaque message. Elle coûte un `sha256` sur
//! quelques kilo-octets — invisible devant une seconde d'appel de modèle — et elle attrape
//! l'altération hors processus : une console `psql`, une restauration partielle, un script
//! d'exploitation. Elle ne remplace pas [`crate::personnage::verifier_integrite`], qui
//! recompose et attrape la dérive éditoriale ; c'en est la moitié qu'on peut se permettre de
//! payer à chaque message.
//!
//! L'empreinte vit dans la même ligne que le texte : la console qui modifie l'un peut modifier
//! l'autre. C'est un contrôle de cohérence, pas un sceau — et c'est déjà ce qui manquait.

use sqlx::PgPool;
use uuid::Uuid;

use super::ErreurBase;
use crate::personnage::sceau::Sceau;

/// À qui l'utilisateur parle, et avec quel texte.
#[derive(Debug, Clone)]
pub struct Compagnon {
    /// Le fil, créé au premier message.
    pub conversation_id: Uuid,
    /// Le prompt système **validé**, tel quel.
    pub prompt_systeme: String,
}

/// L'état de la relation, du point de vue du worker.
///
/// # Pourquoi un seul énuméré plutôt que plusieurs contrôles
///
/// La vérification d'âge était un `if` autonome, avant l'ouverture du compagnon, sans aucun
/// lien de type avec elle. Les deux contrôles n'étaient donc tenus ensemble que par l'**ordre
/// de deux instructions** dans une fonction qui va s'allonger : la phase 2 y insérera le
/// chargement de l'historique et la compaction. C'est le moment précis où un ordre se perd.
///
/// Réunis ici, ils deviennent un `match` exhaustif : le compilateur refuse d'oublier une
/// branche, et il n'existe plus de chemin qui produise un [`Compagnon`] sans les avoir tous
/// franchis.
#[derive(Debug, Clone)]
pub enum Interlocuteur {
    /// Tout est en place.
    Pret(Compagnon),

    /// L'utilisateur n'a pas passé la vérification d'âge.
    ///
    /// Le défaut est sûr : un utilisateur inconnu n'a rien vérifié, donc n'accède à rien.
    AgeNonVerifie,
    /// L'utilisateur n'a pas encore de compagnon actif et validé.
    ///
    /// Un seul cas pour trois causes — aucun compagnon, un brouillon, un prompt non validé —
    /// parce que la conduite à tenir est la même : inviter à en créer un. Les distinguer
    /// devant l'utilisateur exposerait un vocabulaire interne.
    Aucun,
    /// Le prompt stocké ne correspond plus à son empreinte.
    ///
    /// Le modèle n'est **pas** appelé. Ce n'est pas une précaution excessive : le prompt système
    /// est le seul point de contrôle de la modération, et un texte altéré hors processus n'a
    /// franchi aucun contrôle.
    PromptAltere {
        /// Le compagnon concerné, pour que le journal désigne quelque chose.
        personnage_id: Uuid,
    },
}

/// Trouve le compagnon actif de l'utilisateur, vérifie son prompt, et ouvre le fil.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si une lecture ou l'ouverture du fil échoue.
pub async fn ouvrir(
    pool: &PgPool,
    utilisateur_id: Uuid,
    sceau: &Sceau,
) -> Result<Interlocuteur, ErreurBase> {
    // Jointe à la même requête plutôt que demandée séparément : c'est ce qui empêche un futur
    // appelant d'ouvrir un compagnon sans avoir vérifié l'âge. Le coût est nul — la ligne est
    // déjà lue par la clé étrangère.
    if !super::utilisateurs::age_verifie(pool, utilisateur_id).await? {
        return Ok(Interlocuteur::AgeNonVerifie);
    }

    // `statut = 'actif'` **et** `valide_le is not null` : la base garantit déjà que le second
    // découle du premier (migration 0004), mais le worker est le dernier à pouvoir refuser
    // avant que le texte parte au modèle, et une garantie tenue deux fois ne coûte rien ici.
    let trouve: Option<(Uuid, String, String)> = sqlx::query_as(
        "select p.id, m.prompt_systeme_genere, m.prompt_systeme_sceau
           from personnages p
           join personnage_parametres_modele m on m.personnage_id = p.id
          where p.utilisateur_id = $1
            and p.supprime_le is null
            and p.statut = 'actif'
            and m.valide_le is not null",
    )
    .bind(utilisateur_id)
    .fetch_optional(pool)
    .await?;

    let Some((personnage_id, prompt_systeme, empreinte)) = trouve else {
        return Ok(Interlocuteur::Aucun);
    };

    if !sceau.verifier(&prompt_systeme, &empreinte) {
        return Ok(Interlocuteur::PromptAltere { personnage_id });
    }

    let conversation_id = ouvrir_le_fil(pool, utilisateur_id, personnage_id).await?;

    Ok(Interlocuteur::Pret(Compagnon {
        conversation_id,
        prompt_systeme,
    }))
}

/// Rend le fil de l'utilisateur, en le créant au besoin.
///
/// Le `do update` sans effet est ce qui permet un `returning` sur le chemin de conflit : sans
/// lui, une seconde requête serait nécessaire, et la course qu'elle ouvre — deux workers, deux
/// lectures, deux insertions — est précisément celle que l'index unique existe pour interdire.
/// La file garantit déjà qu'un seul worker sert un utilisateur à la fois ; la ligne de commande,
/// elle, ne passe pas par la file.
async fn ouvrir_le_fil(
    pool: &PgPool,
    utilisateur_id: Uuid,
    personnage_id: Uuid,
) -> Result<Uuid, ErreurBase> {
    Ok(sqlx::query_scalar(
        "insert into conversations (utilisateur_id, personnage_id)
         values ($1, $2)
         on conflict (utilisateur_id) where supprime_le is null
         do update set utilisateur_id = excluded.utilisateur_id
         returning id",
    )
    .bind(utilisateur_id)
    .bind(personnage_id)
    .fetch_one(pool)
    .await?)
}

/// Qui a écrit un message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auteur {
    /// L'humain.
    Utilisateur,
    /// Le compagnon.
    Compagnon,
}

impl Auteur {
    /// La valeur acceptée par le `check` de la colonne `role`.
    const fn en_sql(self) -> &'static str {
        match self {
            Self::Utilisateur => "utilisateur",
            Self::Compagnon => "personnage",
        }
    }
}

/// Inscrit un message au fil, et rend son identifiant.
///
/// `identifiant_telegram` est celui du message chez Telegram, quand il est connu : il permet de
/// relier une ligne d'ici à ce que l'utilisateur voit, ce dont un signalement a besoin.
///
/// # Pourquoi l'écriture est idempotente
///
/// Une tâche reprise repasse par le début : sans cela, le message entrant était réinscrit à
/// chaque tentative. Mesuré sur un modèle qui expire, `messages` finissait avec **trois copies**
/// de ce que la personne avait écrit une fois.
///
/// La conséquence immédiate est mineure. Celle de la phase 2 ne l'est pas : c'est cette table
/// qui composera l'historique envoyé au modèle, et un incident réseau d'aujourd'hui deviendrait
/// un tour de conversation dupliqué des semaines plus tard.
///
/// Telegram fournit la clé d'idempotence — l'identifiant du message. Les messages du compagnon
/// n'en ont pas tant qu'ils ne sont pas partis, d'où l'index partiel : eux ne conflictent
/// jamais, et c'est voulu.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn inscrire_message(
    pool: &PgPool,
    conversation_id: Uuid,
    auteur: Auteur,
    contenu: &str,
    identifiant_telegram: Option<i64>,
) -> Result<Uuid, ErreurBase> {
    let mut tx = pool.begin().await?;

    let deja_vu: Option<Uuid> = sqlx::query_scalar(
        "insert into messages (conversation_id, role, contenu, identifiant_telegram)
         values ($1, $2, $3, $4)
         on conflict (conversation_id, identifiant_telegram)
             where identifiant_telegram is not null
         do nothing
         returning id",
    )
    .bind(conversation_id)
    .bind(auteur.en_sql())
    .bind(contenu)
    .bind(identifiant_telegram)
    .fetch_optional(&mut *tx)
    .await?;

    // `do nothing` ne rend aucune ligne sur conflit : il faut alors relire celle qui existe.
    // Rendre l'identifiant de la ligne déjà présente plutôt qu'une erreur est ce qui fait de
    // la reprise une opération sans effet, et non un échec.
    let id = match deja_vu {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "select id from messages
                  where conversation_id = $1 and identifiant_telegram = $2",
            )
            .bind(conversation_id)
            .bind(identifiant_telegram)
            .fetch_one(&mut *tx)
            .await?
        }
    };

    // Dans la même transaction que le message : `dernier_message_le` sert à décider quand le
    // compagnon reprend l'initiative (phase 2). S'il pouvait diverger du dernier message réel,
    // il déclencherait des relances à contretemps — et c'est le genre d'écart qui ne se
    // constate qu'en production, chez quelqu'un.
    sqlx::query("update conversations set dernier_message_le = now() where id = $1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(id)
}

/// Une réponse déjà produite qui n'est jamais partie.
#[derive(Debug, Clone)]
pub struct ReponseEnAttente {
    /// La ligne à confirmer une fois l'envoi abouti.
    pub message_id: Uuid,
    /// Le texte à renvoyer, tel qu'il a été généré.
    pub texte: String,
}

/// Cherche une réponse déjà générée pour ce message, et pas encore délivrée.
///
/// # Ce que cette fonction fait gagner
///
/// Une réponse produite dont l'envoi échoue — un `502` de Telegram, un délai — était jetée : la
/// tâche repartait du début et **rappelait le modèle**. Trois générations facturées pour une
/// panne qui n'a rien à voir avec le modèle, mesuré.
///
/// # Comment une réponse en attente se distingue d'une vieille
///
/// Par sa date, comparée à celle du message auquel elle répond. Une reprise réinscrit le message
/// entrant de façon idempotente, donc `apres` garde la date de la **première** tentative : la
/// réponse de cette tâche lui est postérieure. La réponse orpheline d'une tâche abandonnée, elle,
/// est antérieure au message suivant — et ne sera donc jamais renvoyée à contretemps.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si la lecture échoue.
pub async fn reponse_a_renvoyer(
    pool: &PgPool,
    conversation_id: Uuid,
    apres: Uuid,
) -> Result<Option<ReponseEnAttente>, ErreurBase> {
    Ok(sqlx::query_as(
        "select m.id, coalesce(m.contenu, '')
           from messages m
          where m.conversation_id = $1
            and m.role = 'personnage'
            and m.identifiant_telegram is null
            and m.cree_le > (select cree_le from messages where id = $2)
          order by m.cree_le desc
          limit 1",
    )
    .bind(conversation_id)
    .bind(apres)
    .fetch_optional(pool)
    .await?
    .map(|(message_id, texte)| ReponseEnAttente { message_id, texte }))
}

/// Note qu'une réponse est bien parvenue, en lui attachant son identifiant Telegram.
///
/// C'est ce qui donne son sens à la colonne : `identifiant_telegram` non nul signifie **« la
/// personne l'a reçu »**. La mémoire de la phase 2 devra ne lire que ces lignes-là — une
/// réponse générée et jamais délivrée n'a pas eu lieu dans la conversation.
///
/// # Errors
///
/// [`ErreurBase::Requete`] si l'écriture échoue.
pub async fn confirmer_envoi(
    pool: &PgPool,
    message_id: Uuid,
    identifiant_telegram: Option<i64>,
) -> Result<(), ErreurBase> {
    sqlx::query("update messages set identifiant_telegram = $2 where id = $1")
        .bind(message_id)
        .bind(identifiant_telegram)
        .execute(pool)
        .await?;
    Ok(())
}
