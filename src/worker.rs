//! Les consommateurs de la file, et ce qu'ils font d'une tâche.
//!
//! # Pourquoi le webhook ne répond pas lui-même
//!
//! Telegram attend une réponse HTTP et rejoue la mise à jour si elle tarde. Répondre dans le
//! gestionnaire marcherait tant que la réponse est un écho, et cesserait de marcher dès qu'elle
//! coûte un appel de modèle — puis une génération d'image qui se compte en minutes.
//!
//! # Concurrence : entre les conversations, pas dans une conversation
//!
//! Plusieurs workers tournent en parallèle. Ce n'était pas le cas en phase 0, où le traitement
//! sérialisé garantissait gratuitement l'ordre des réponses — et où l'écho coûtait cinquante
//! millisecondes. Dès qu'une réponse coûte des secondes, ce même sérialisme fait attendre la
//! centième personne pendant cinq minutes, sans qu'aucune erreur ne soit journalisée : le bot
//! paraît simplement mort.
//!
//! L'ordre reste tenu là où il compte — dans une conversation — par la requête de prise, qui
//! écarte tout utilisateur déjà servi ailleurs (voir [`crate::db::file`]). Le worker n'a donc
//! aucune synchronisation à faire : la base la lui donne.
//!
//! # Pourquoi une scrutation et pas une notification
//!
//! Les workers interrogent la file à intervalle court plutôt que d'être réveillés par un
//! `LISTEN/NOTIFY`. C'est un compromis assumé pour cette phase : la latence ajoutée est bornée
//! par `REPOS_MAX` (250 ms), et l'absence de canal de notification retire une pièce mobile au moment
//! où la file elle-même est neuve. À reprendre quand la latence comptera davantage que la
//! simplicité — c'est-à-dire quand la réponse ne sera plus un écho.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::dialogue::{Auteur, Interlocuteur};
use crate::db::{Base, consommation, dialogue, file};
use crate::error::ErrorCode;
use crate::modele::{ClientModele, ContexteConversation, ErreurModele, Role, Tour};
use crate::personnage::sceau::Sceau;
use crate::telegram::Canal;
use crate::telegram::envoi::Action;
use crate::telegram::types::Recu;

/// Nombre de consommateurs lancés en parallèle.
///
/// Quatre plutôt qu'un par cœur : le travail est presque entièrement de l'attente réseau, pas
/// du calcul. La borne réelle est ailleurs — le débit que Telegram accepte, et le nombre de
/// connexions du pool.
pub const WORKERS: usize = 4;

/// Durée du bail posé sur une tâche prise.
///
/// Généreuse par rapport au coût d'un écho, parce qu'elle doit couvrir le cas le plus lent, pas
/// le plus fréquent : un bail trop court ferait reprendre par un second worker une tâche que le
/// premier est encore en train de traiter, et l'utilisateur recevrait deux réponses.
const BAIL: Duration = Duration::from_secs(120);

/// Repos après un échec de tâche, avant de reprendre la boucle.
///
/// **Ce n'est pas de la décoration, et sa suppression casse une garantie.** Une tâche qui
/// échoue est aussitôt reprenable ; sans ce frein, les quatre workers épuisent ses trois
/// tentatives en quelques millisecondes, et une panne passagère de Telegram — quelques
/// secondes — suffit à faire abandonner définitivement un message qui serait passé au coup
/// suivant. Le test de survie l'attrape : sans ce repos, la file finit en `echec` là où elle
/// devrait finir en `en_attente`.
///
/// Il n'est PAS payé après un succès : la file vient alors de prouver qu'elle a du travail, et
/// dormir y coûtait la moitié du cycle (mesuré : 25 ms sur 50 ms de bout en bout).
const REPOS_APRES_ECHEC: Duration = Duration::from_millis(25);

/// Repos quand la file est vide. Borne haute de la latence ajoutée par la scrutation.
const REPOS_MAX: Duration = Duration::from_millis(250);

/// Ce que reçoit quelqu'un qui n'a pas encore de compagnon actif.
///
/// Un seul message pour trois causes internes — aucun compagnon, un brouillon, un prompt non
/// validé — parce que la conduite à tenir est la même. Les distinguer exposerait un vocabulaire
/// interne à quelqu'un qui n'a rien demandé de tel.
const AUCUN_COMPAGNON: &str = "Tu n'as pas encore d'assistant.\n\n\
     La création n'est pas encore ouverte depuis Telegram — elle arrive. \
     En attendant, il n'y a personne pour te répondre.";

/// Ce que reçoit quelqu'un dont le compagnon est momentanément indisponible.
///
/// Ne nomme ni le modèle, ni le fournisseur, ni le code d'erreur : ce sont des mots qui
/// n'appartiennent pas à cette conversation. Ne joue pas non plus le personnage — faire dire au
/// compagnon « je suis fatigué » pour masquer une panne serait un mensonge, et le produit tout
/// entier repose sur ce que l'utilisateur croit de lui.
const INDISPONIBLE: &str = "Je n'arrive pas à répondre pour le moment. Réessaie dans un instant.";

/// Ce que reçoit quelqu'un dont l'âge n'est pas vérifié.
///
/// Un refus muet serait indiscernable d'une panne — c'est la première friction que la carte des
/// parcours signale. Le message dit ce qui manque, sans jouer de personnage : la vérification
/// d'âge est une limite de service, et la présenter autrement serait malhonnête.
const VERIFICATION_REQUISE: &str = "Avant de commencer, ce service demande une vérification d'âge.\n\n\
     Elle n'est pas encore disponible — cette phase met en place la persistance. \
     Reviens quand l'inscription sera ouverte.";

/// Les consommateurs, et ce qui les arrête.
///
/// # Pourquoi un type et pas trois valeurs libres
///
/// Le lancement et l'extinction étaient recopiés dans les deux portes d'entrée — service
/// webhook et scrutation — sous la forme d'un `watch::Sender`, d'un `Vec<JoinHandle>` et d'une
/// constante que chaque appelant devait assembler puis démonter dans le bon ordre. Les deux
/// copies avaient **déjà divergé** le jour de leur écriture : l'une journalisait le lancement,
/// l'autre non ; les messages d'échec différaient. Un type rend l'oubli impossible plutôt que
/// surveillé — c'est le même raisonnement qui a mis l'admission en commun.
pub struct Equipe {
    arret: watch::Sender<bool>,
    taches: Vec<JoinHandle<()>>,
    /// Décrémenté par chaque worker en sortant, pour que la sonde dise ce qui **tourne** et non
    /// ce qui a été lancé.
    vivants: Arc<AtomicUsize>,
}

impl Equipe {
    /// Lance [`WORKERS`] consommateurs sur cette base.
    ///
    /// Le client de modèle est partagé, pas dupliqué : il porte un pool de connexions HTTP que
    /// les quatre workers ont intérêt à réutiliser, et un secret qu'on ne recopie pas quatre
    /// fois sur le tas.
    ///
    /// Il est reçu en paramètre plutôt que construit ici, ce qui est la raison d'être du trait :
    /// les tests injectent un double qui fabrique les pannes du fournisseur, et le worker est
    /// alors éprouvé sur des échecs qui, en production, n'arrivent qu'au pire moment.
    #[must_use]
    pub fn lancer(
        base: &Base,
        canal: &Arc<Canal>,
        modele: &Arc<dyn ClientModele>,
        sceau: &Arc<Sceau>,
    ) -> Self {
        let (arret, ecoute) = watch::channel(false);
        let vivants = Arc::new(AtomicUsize::new(WORKERS));
        let taches = (0..WORKERS)
            .map(|numero| {
                tokio::spawn(tourner(
                    base.clone(),
                    Arc::clone(canal),
                    Arc::clone(modele),
                    Arc::clone(sceau),
                    ecoute.clone(),
                    numero,
                    Arc::clone(&vivants),
                ))
            })
            .collect();
        tracing::info!(workers = WORKERS, "consommateurs lancés");
        Self {
            arret,
            taches,
            vivants,
        }
    }

    /// Le nombre de consommateurs encore en vie, partageable avec la sonde.
    #[must_use]
    pub fn vivants(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.vivants)
    }

    /// Demande l'arrêt et attend la fin des tâches en cours, sous une borne.
    ///
    /// Attend chaque worker l'un après l'autre plutôt qu'avec un `join_all` : ils sortent tous
    /// sur le même signal, donc l'attente totale est celle du plus lent dans les deux cas — et
    /// cela évite d'ajouter une dépendance pour une seule ligne.
    pub async fn eteindre(self, delai: Duration) {
        tracing::info!(delai_max = ?delai, "fin des tâches en cours");
        let _ = self.arret.send(true);

        let attente = async {
            let mut interrompus = 0_usize;
            for tache in self.taches {
                if tache.await.is_err() {
                    interrompus += 1;
                }
            }
            interrompus
        };
        match tokio::time::timeout(delai, attente).await {
            Ok(0) => tracing::info!("tâches en cours terminées, arrêt propre"),
            Ok(interrompus) => {
                tracing::error!(interrompus, "des workers se sont interrompus anormalement");
            }
            // Ce qui reste en file survit à l'arrêt : la borne ne perd rien, elle empêche
            // seulement un worker bloqué de retenir le processus.
            Err(_) => tracing::error!(
                delai = ?delai,
                "des tâches en cours n'ont pas fini ; elles seront reprises au bail"
            ),
        }
    }
}

/// Consomme la file jusqu'à ce que l'arrêt soit demandé.
///
/// Termine la tâche en cours avant de rendre la main : une tâche interrompue serait reprise par
/// le bail, mais l'utilisateur recevrait sa réponse deux fois.
async fn tourner(
    base: Base,
    canal: Arc<Canal>,
    modele: Arc<dyn ClientModele>,
    sceau: Arc<Sceau>,
    mut arret: watch::Receiver<bool>,
    numero: usize,
    vivants: Arc<AtomicUsize>,
) {
    tracing::debug!(numero, "worker démarré");
    let mut traites: u64 = 0;

    loop {
        if *arret.borrow() {
            break;
        }

        // Un seul point d'attente pour toute la boucle : chaque branche dit combien de temps
        // se reposer, et l'attente est faite une fois, au même endroit, toujours interruptible.
        let repos = match file::prendre(base.pool(), BAIL).await {
            Ok(Some(tache)) => {
                let issue = traiter(&base, &canal, modele.as_ref(), &sceau, &tache).await;
                traites += 1;
                match issue {
                    // Rien à attendre : la file vient de prouver qu'elle a du travail.
                    Issue::Close => continue,
                    Issue::Echouee => REPOS_APRES_ECHEC,
                }
            }
            // Rien de prenable : soit la file est vide, soit tout ce qu'elle contient appartient
            // à des utilisateurs déjà servis ailleurs.
            Ok(None) => REPOS_MAX,
            Err(erreur) => {
                tracing::error!(numero, %erreur, "file inaccessible");
                REPOS_MAX
            }
        };

        tokio::select! {
            biased;
            _ = arret.changed() => break,
            () = tokio::time::sleep(repos) => {}
        }
    }

    vivants.fetch_sub(1, Ordering::Relaxed);
    tracing::debug!(numero, traites, "worker arrêté");
}

/// Ce qu'il est advenu d'une tâche traitée, du seul point de vue qui intéresse la boucle.
enum Issue {
    /// Close, quelle qu'en soit la raison — succès, ou refus définitif de Telegram.
    Close,
    /// Remise en file : la boucle doit freiner avant de la reprendre.
    Echouee,
}

/// Traite une tâche prise, et la rend à la file dans tous les cas.
async fn traiter(
    base: &Base,
    canal: &Canal,
    modele: &dyn ClientModele,
    sceau: &Sceau,
    tache: &file::Tache,
) -> Issue {
    let Ok(recu) = serde_json::from_value::<Recu>(tache.charge_utile.clone()) else {
        tracing::error!(
            tache = %tache.id,
            code = ErrorCode::TacheIllisible.code(),
            "charge utile illisible, tâche abandonnée"
        );
        rendre_en_echec(base, tache, ErrorCode::TacheIllisible).await;
        return Issue::Echouee;
    };

    // Un seul point de décision, et un `match` exhaustif : la vérification d'âge, l'existence
    // d'un compagnon actif et l'intégrité de son prompt sont désormais trois issues d'une même
    // lecture. Aucune ne peut être oubliée par un chemin futur — la phase 2 va allonger cette
    // fonction.
    let compagnon = match dialogue::ouvrir(base.pool(), tache.utilisateur_id, sceau).await {
        Ok(Interlocuteur::Pret(compagnon)) => compagnon,
        Ok(Interlocuteur::AgeNonVerifie) => {
            tracing::info!(chat_id = recu.chat_id, "âge non vérifié, accès au moteur refusé");
            return conclure(base, canal, tache, &recu, VERIFICATION_REQUISE).await;
        }
        Ok(Interlocuteur::Aucun) => {
            tracing::info!(chat_id = recu.chat_id, "aucun compagnon actif");
            return conclure(base, canal, tache, &recu, AUCUN_COMPAGNON).await;
        }
        Ok(Interlocuteur::PromptAltere { personnage_id }) => {
            // Le prompt système est le seul point de contrôle de la modération. Un texte qui ne
            // correspond plus à son empreinte n'a franchi aucun contrôle, et une reprise le
            // trouverait tout aussi altéré : la tâche est close, pas remise en file.
            tracing::error!(
                %personnage_id,
                code = ErrorCode::Interne.code(),
                "prompt système altéré hors processus, appel au modèle refusé"
            );
            return conclure(base, canal, tache, &recu, INDISPONIBLE).await;
        }
        Err(erreur) => {
            tracing::error!(tache = %tache.id, %erreur, "compagnon illisible");
            rendre_en_echec(base, tache, ErrorCode::Interne).await;
            return Issue::Echouee;
        }
    };

    // Le message entrant est inscrit AVANT l'appel : s'il échoue, ce que la personne a écrit
    // reste. Le perdre serait le pire des échecs — bien pire que l'absence de réponse.
    let entrant = match dialogue::inscrire_message(
        base.pool(),
        compagnon.conversation_id,
        Auteur::Utilisateur,
        &recu.texte,
        Some(recu.message_id),
    )
    .await
    {
        Ok(id) => id,
        Err(erreur) => {
            tracing::error!(tache = %tache.id, %erreur, "message entrant non inscrit");
            rendre_en_echec(base, tache, ErrorCode::Interne).await;
            return Issue::Echouee;
        }
    };

    // Une reprise ne régénère pas. Si une réponse a déjà été produite pour ce message et n'est
    // jamais partie — Telegram a refusé, le réseau a coupé — c'est elle qu'on renvoie. Sans
    // cela, une panne d'envoi faisait repayer trois générations complètes pour un incident qui
    // n'a rien à voir avec le modèle.
    match dialogue::reponse_a_renvoyer(base.pool(), compagnon.conversation_id, entrant).await {
        Ok(Some(en_attente)) => {
            tracing::info!(
                chat_id = recu.chat_id,
                tentative = tache.tentatives,
                "réponse déjà produite, renvoyée sans rappeler le modèle"
            );
            return renvoyer(base, canal, tache, &recu, &en_attente).await;
        }
        Ok(None) => {}
        Err(erreur) => {
            // Ne pas savoir s'il y a une réponse en attente n'empêche pas d'en produire une ;
            // au pire on paie une génération de plus, ce qui est l'ancien comportement.
            tracing::warn!(tache = %tache.id, %erreur, "réponse en attente illisible");
        }
    }

    // L'indication d'activité est un confort : son échec ne doit pas empêcher la réponse. Elle
    // compte davantage qu'en phase 1.1 — l'écho partait en cinquante millisecondes, un modèle
    // met des secondes, et pendant ces secondes elle est tout ce que la personne voit.
    if let Err(erreur) = canal.action(recu.chat_id, Action::Typing).await {
        tracing::debug!(chat_id = recu.chat_id, %erreur, "indication d'activité non affichée");
    }

    let contexte = ContexteConversation {
        prompt_systeme: compagnon.prompt_systeme.clone(),
        echanges: vec![Tour {
            role: Role::Utilisateur,
            texte: recu.texte.clone(),
        }],
    };

    // Aucune transaction n'est tenue pendant cet appel : le pool est dimensionné pour seize
    // connexions et quatre workers, et retenir une connexion pendant une seconde de calcul GPU
    // renverserait ce raisonnement (voir `db::CONNEXIONS_MAX`).
    let reponse = match modele.repondre(&contexte).await {
        Ok(reponse) => reponse,
        Err(erreur) => {
            // Écrite ici, à côté des deux autres inscriptions : « un appel payé, une ligne »
            // se lit alors dans `traiter`, et non éparpillé chez les fonctions qui décident
            // d'autre chose.
            inscrire_au_registre(base, modele, tache, &recu, &compagnon, None, None).await;
            return echec_du_modele(base, canal, tache, &recu, &erreur).await;
        }
    };

    if reponse.tronquee {
        // Envoyée quand même : une phrase inachevée vaut mieux qu'un silence. Mais elle se voit,
        // et la cause est un réglage — pas une panne.
        tracing::warn!(
            chat_id = recu.chat_id,
            unites_sortie = ?reponse.unites_sortie,
            "réponse coupée par la limite de jetons"
        );
    }

    let cout_eur = modele.cout_eur(reponse.unites_entree, reponse.unites_sortie);
    tracing::info!(
        chat_id = recu.chat_id,
        modele = %reponse.modele,
        duree_ms = reponse.duree.as_millis(),
        unites_sortie = ?reponse.unites_sortie,
        cout_eur = %cout_eur,
        "réponse produite"
    );

    // La réponse est inscrite AVANT l'envoi, sans identifiant Telegram. C'est ce qui la rend
    // renvoyable si l'envoi échoue, au lieu d'être régénérée.
    //
    // La colonne porte alors la distinction : `identifiant_telegram` non nul signifie « la
    // personne l'a reçu ». La mémoire de la phase 2 devra ne lire que ces lignes-là — une
    // réponse générée et jamais délivrée n'a pas eu lieu dans la conversation.
    let message_id = match dialogue::inscrire_message(
        base.pool(),
        compagnon.conversation_id,
        Auteur::Compagnon,
        &reponse.texte,
        None,
    )
    .await
    {
        Ok(id) => Some(id),
        Err(erreur) => {
            // On perd la trace, pas le message : la réponse part quand même. Elle ne sera
            // simplement pas renvoyable si l'envoi échoue.
            tracing::error!(tache = %tache.id, %erreur, "réponse produite mais non inscrite");
            None
        }
    };

    // Le modèle a été payé, que Telegram accepte ou non — et il ne sera pas rappelé sur reprise.
    inscrire_au_registre(base, modele, tache, &recu, &compagnon, message_id, Some(&reponse)).await;

    livrer(base, canal, tache, &recu, message_id, &reponse.texte).await
}

/// Renvoie une réponse déjà produite, sans repasser par le modèle.
async fn renvoyer(
    base: &Base,
    canal: &Canal,
    tache: &file::Tache,
    recu: &Recu,
    en_attente: &dialogue::ReponseEnAttente,
) -> Issue {
    // L'indication d'activité est renvoyée aussi : de l'extérieur, une reprise doit ressembler à
    // un service qui répond, pas à un service qui hésite.
    if let Err(erreur) = canal.action(recu.chat_id, Action::Typing).await {
        tracing::debug!(chat_id = recu.chat_id, %erreur, "indication d'activité non affichée");
    }
    livrer(
        base,
        canal,
        tache,
        recu,
        Some(en_attente.message_id),
        &en_attente.texte,
    )
    .await
}

/// Envoie un texte déjà produit, confirme sa réception, et rend la tâche.
///
/// Partagée par la production et le renvoi : les deux ont exactement le même épilogue, et
/// l'écrire deux fois l'aurait fait diverger — c'est ce que les commentaires de `clore` et
/// `rendre_en_echec` racontent déjà pour d'autres gestes.
async fn livrer(
    base: &Base,
    canal: &Canal,
    tache: &file::Tache,
    recu: &Recu,
    message_id: Option<uuid::Uuid>,
    texte: &str,
) -> Issue {
    match canal.envoyer_texte(recu.chat_id, texte).await {
        Ok(identifiants) => {
            tracing::info!(
                chat_id = recu.chat_id,
                message_id = recu.message_id,
                morceaux = identifiants.len(),
                "réponse envoyée"
            );
            if let Some(id) = message_id
                && let Err(erreur) =
                    dialogue::confirmer_envoi(base.pool(), id, identifiants.first().copied()).await
            {
                // La personne a sa réponse. Ne pas confirmer laisse la ligne renvoyable, mais la
                // tâche est close juste après : personne ne viendra la relire.
                tracing::error!(tache = %tache.id, %erreur, "réception non confirmée");
            }
            clore(base, tache, "tâche traitée mais non close").await;
            Issue::Close
        }
        Err(erreur) if erreur.merite_une_reprise() => {
            tracing::warn!(
                chat_id = recu.chat_id,
                tentative = tache.tentatives,
                attente = ?erreur.attendre(),
                %erreur,
                "réponse non envoyée, tâche remise en file"
            );
            rendre_en_echec(base, tache, ErrorCode::EnvoiImpossible).await;
            Issue::Echouee
        }
        Err(erreur) => {
            // Un utilisateur qui bloque le bot n'est pas un incident, et réessayer referait
            // exactement la même erreur : la tâche est close, pas reprise.
            tracing::info!(chat_id = recu.chat_id, %erreur, "refus définitif de Telegram");
            clore(base, tache, "tâche abandonnée mais non close").await;
            Issue::Close
        }
    }
}

/// Envoie un message de service et clôt la tâche.
///
/// Les trois refus — âge non vérifié, aucun compagnon, prompt altéré — partageaient le même
/// épilogue : envoyer, journaliser, clore. Écrit trois fois, il aurait divergé trois fois ; et
/// c'est justement sur ces chemins-là, les moins parcourus, qu'une divergence ne se voit pas.
///
/// Le refus lui-même n'est jamais repris — réessayer referait exactement le même refus. Mais
/// l'**envoi** l'est, exactement comme sur le chemin principal : une coupure réseau ne doit pas
/// faire disparaître le message qui explique à quelqu'un pourquoi il n'a pas de réponse. Une
/// première version clôturait la tâche quoi qu'il arrive, et perdait donc silencieusement le
/// seul message qui distinguait un refus d'une panne.
async fn conclure(
    base: &Base,
    canal: &Canal,
    tache: &file::Tache,
    recu: &Recu,
    texte: &str,
) -> Issue {
    match canal.envoyer_texte(recu.chat_id, texte).await {
        Ok(_) => {
            clore(base, tache, "message de service envoyé mais tâche non close").await;
            Issue::Close
        }
        Err(erreur) if erreur.merite_une_reprise() => {
            tracing::warn!(
                chat_id = recu.chat_id,
                tentative = tache.tentatives,
                %erreur,
                "message de service non envoyé, tâche remise en file"
            );
            rendre_en_echec(base, tache, ErrorCode::EnvoiImpossible).await;
            Issue::Echouee
        }
        Err(erreur) => {
            tracing::info!(chat_id = recu.chat_id, %erreur, "refus définitif de Telegram");
            clore(base, tache, "message de service abandonné mais tâche non close").await;
            Issue::Close
        }
    }
}

/// Décide de la suite quand le modèle n'a pas répondu.
///
/// Deux questions, dans cet ordre : l'erreur mérite-t-elle une reprise, et reste-t-il des
/// tentatives ? La seconde compte autant que la première — sans elle, la dernière tentative
/// d'une panne passagère laisse la personne devant un silence, ce qui est indiscernable d'un
/// bot mort.
///
/// Le passage de `tentatives_max = 0` à [`file::echouer`] n'est pas une astuce : la file dit
/// « échec définitif quand `tentatives >= max` », et zéro est la façon d'exprimer « ne rejoue
/// pas » dans le vocabulaire qui existe déjà, plutôt que d'ajouter une seconde fonction.
async fn echec_du_modele(
    base: &Base,
    canal: &Canal,
    tache: &file::Tache,
    recu: &Recu,
    erreur: &ErreurModele,
) -> Issue {
    let rejouable = erreur.merite_une_reprise();
    let reste_des_tentatives = tache.tentatives < file::TENTATIVES_MAX;

    if rejouable && reste_des_tentatives {
        tracing::warn!(
            chat_id = recu.chat_id,
            tentative = tache.tentatives,
            %erreur,
            "modèle indisponible, tâche remise en file"
        );
        rendre_en_echec(base, tache, ErrorCode::Interne).await;
        return Issue::Echouee;
    }

    tracing::error!(
        chat_id = recu.chat_id,
        tentative = tache.tentatives,
        rejouable,
        %erreur,
        "modèle indisponible, abandon définitif"
    );
    if let Err(erreur_envoi) = canal.envoyer_texte(recu.chat_id, INDISPONIBLE).await {
        tracing::warn!(chat_id = recu.chat_id, %erreur_envoi, "excuse non envoyée");
    }
    // Abandon franc : la personne vient d'être prévenue, la reprendre lui écrirait deux fois.
    if let Err(erreur_file) =
        file::abandonner(base.pool(), tache.id, i32::from(ErrorCode::Interne.code())).await
    {
        tracing::error!(tache = %tache.id, %erreur_file, "tâche abandonnée et non rendue");
    }
    Issue::Close
}

/// Inscrit une ligne au registre des coûts, en journalisant si l'écriture échoue.
///
/// Ne rend pas d'erreur, et c'est délibéré : un registre qui refuse une écriture ne doit pas
/// faire perdre à quelqu'un une réponse déjà produite et déjà payée. La perte est journalisée
/// pour se retrouver dans l'écart avec la facture du fournisseur.
///
/// Le statut et le coût sont **dérivés** de la présence d'une réponse, et non passés. Ils
/// l'étaient, et les trois sites d'appel obéissaient alors à une règle qu'aucun d'eux
/// n'énonçait : « une réponse ⇒ `Ok` et son coût, pas de réponse ⇒ `Echec` et zéro ». Le jour
/// où un quatrième appelant aurait passé une réponse avec `Echec`, le registre aurait menti
/// sans qu'aucun test ne l'attrape — la cohérence à deux couches que ce projet traque ailleurs.
async fn inscrire_au_registre(
    base: &Base,
    modele: &dyn ClientModele,
    tache: &file::Tache,
    recu: &Recu,
    compagnon: &dialogue::Compagnon,
    message_id: Option<uuid::Uuid>,
    reponse: Option<&crate::modele::ReponseModele>,
) {
    let cout_eur = reponse.map_or(rust_decimal::Decimal::ZERO, |r| {
        modele.cout_eur(r.unites_entree, r.unites_sortie)
    });
    let statut = if reponse.is_some() {
        consommation::Statut::Ok
    } else {
        // Un appel qui échoue après le début de la génération est souvent facturé. Le montant
        // réel est inconnu — le fournisseur n'a rien rendu — mais la ligne existe, ce qui rend
        // l'échec visible dans le registre au lieu de le laisser invisible dans la marge.
        consommation::Statut::Echec
    };

    let appel = consommation::Appel {
        utilisateur_id: tache.utilisateur_id,
        conversation_id: Some(compagnon.conversation_id),
        message_id,
        type_appel: consommation::TypeAppel::Message,
        origine: consommation::Origine::Reponse,
        fournisseur: modele.fournisseur(),
        // L'identifiant rendu par le fournisseur, jamais celui demandé : mesuré, un serveur
        // peut répondre avec un autre modèle sans le dire. « inconnu » quand l'appel a échoué
        // avant toute réponse — la colonne est `not null`, et mentir un nom serait pire.
        modele: reponse.map_or("inconnu", |r| r.modele.as_str()),
        unites_entree: reponse.and_then(|r| r.unites_entree),
        unites_sortie: reponse.and_then(|r| r.unites_sortie),
        cout_eur,
        duree: reponse.map(|r| r.duree),
        statut,
    };

    if let Err(erreur) = consommation::inscrire(base.pool(), &appel).await {
        tracing::error!(
            chat_id = recu.chat_id,
            %erreur,
            "appel non inscrit au registre des coûts"
        );
    }
}

/// Clôt une tâche, en journalisant si même cela échoue.
///
/// Pendant symétrique de [`rendre_en_echec`] : sans lui, le même geste était écrit deux fois
/// avec deux messages divergents, et un lecteur cherchait un `clore` qui n'existait pas.
///
/// Si la clôture échoue, la tâche sera reprise au bail et renverra la même réponse. C'est le
/// seul point où un doublon reste possible, et il vaut mieux qu'une réponse perdue.
async fn clore(base: &Base, tache: &file::Tache, motif: &'static str) {
    if let Err(erreur) = file::terminer(base.pool(), tache.id).await {
        tracing::error!(tache = %tache.id, %erreur, motif);
    }
}

/// Rend une tâche en échec, en journalisant si même cela échoue.
async fn rendre_en_echec(base: &Base, tache: &file::Tache, code: ErrorCode) {
    if let Err(erreur) = file::echouer(base.pool(), tache.id, i32::from(code.code())).await {
        tracing::error!(tache = %tache.id, %erreur, "tâche en échec et non rendue");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn recu_de(texte: &str) -> Recu {
        Recu {
            chat_id: 42,
            utilisateur_telegram: 42,
            message_id: 17,
            prenom: "Erwan".to_owned(),
            texte: texte.to_owned(),
            recu_le: 1_760_000_000,
        }
    }

    #[test]
    fn les_messages_de_service_ne_jouent_jamais_le_personnage() {
        // Tout ce que le worker peut dire de lui-même, plutôt qu'à travers le compagnon. La
        // règle est unique et vaut pour les trois : ne pas emprunter la voix du compagnon pour
        // annoncer une limite de service ou une panne. Faire dire « je suis fatigué » à un
        // personnage pour masquer un fournisseur injoignable serait un mensonge — et le produit
        // entier repose sur ce que la personne croit de lui.
        for (nom, texte) in [
            ("âge non vérifié", VERIFICATION_REQUISE),
            ("aucun compagnon", AUCUN_COMPAGNON),
            ("indisponible", INDISPONIBLE),
        ] {
            println!("--- {nom} ---\n{texte}\n");
            assert!(!texte.is_empty());
            // Aucun vocabulaire interne : ces mots n'appartiennent pas à la conversation.
            for interdit in ["modèle", "fournisseur", "worker", "tâche", "prompt", "erreur"] {
                assert!(
                    !texte.to_lowercase().contains(interdit),
                    "« {interdit} » n'a rien à faire dans le message « {nom} »"
                );
            }
        }
        assert!(VERIFICATION_REQUISE.contains("vérification d'âge"));
        assert!(AUCUN_COMPAGNON.contains("assistant"));
        assert!(INDISPONIBLE.contains("Réessaie"));
    }

    #[test]
    fn un_recu_survit_a_un_aller_retour_par_la_base() {
        // La charge utile transite en `jsonb` : ce qui ressort doit être ce qui est entré,
        // sinon une tâche reprise après un redémarrage répondrait à côté.
        let avant = recu_de("un aller-retour en jsonb, avec des accents et un emoji 🙂");
        let json = serde_json::to_value(&avant).expect("Recu sérialisable");
        println!("charge utile : {json}");
        let apres: Recu = serde_json::from_value(json).expect("Recu relisible");
        println!("texte relu   : {}", apres.texte);
        assert_eq!(apres.texte, avant.texte);
        assert_eq!(apres.chat_id, avant.chat_id);
        assert_eq!(apres.utilisateur_telegram, avant.utilisateur_telegram);
        assert_eq!(apres.message_id, avant.message_id);
    }

}
