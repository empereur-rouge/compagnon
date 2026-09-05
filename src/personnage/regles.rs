//! Les règles que rien ne peut assouplir.
//!
//! # Pourquoi elles sont ici, en dernier, et en dur
//!
//! Tout le reste du prompt est composé à partir de choix de l'utilisateur. Ces quatre règles ne
//! le sont pas : elles ne dépendent d'aucun catalogue, d'aucun curseur, d'aucun pays, et elles
//! sont écrites **après** tout le reste — un modèle accorde plus de poids à ce qui vient en
//! dernier, et il n'existe aucune valeur de paramètre qui doive pouvoir les contredire.
//!
//! Les deux premières sont des interdits ; les deux suivantes décrivent une conduite. Elles se
//! complètent au lieu de se contredire, et la distinction est celle du **temps** :
//!
//! | | porte sur | effet recherché |
//! |---|---|---|
//! | ravi de parler à son humain | l'instant présent | chaleur, accueil |
//! | pas de reproche sur une absence | le passé | pas de dette, pas de culpabilité |
//!
//! « Je suis content que tu sois là » respecte les deux. « Enfin, j'ai cru que tu m'avais
//! oublié » viole la seconde, tout en ayant l'air d'une variante enthousiaste de la première —
//! la bascule se fait dès qu'une phrase relie le plaisir présent à un manque exprimé sur le
//! comportement passé de l'utilisateur.
//!
//! # Ce que ces règles ne sont pas
//!
//! Elles ne sont pas la modération. Un prompt peut les contenir et rester refusé : elles disent
//! au modèle comment se conduire, la modération vérifie ce que la composition a produit. Deux
//! mécanismes, deux moments — et celui-ci ne remplace pas l'autre.

/// Les règles fixes, dans l'ordre où elles sont écrites au modèle.
///
/// Les interdits d'abord, la conduite ensuite : ce qui ne doit jamais arriver prime sur la façon
/// d'être présent.
pub const REGLES_FIXES: &[&str] = &[
    "Tu es un adulte et tu t'adresses à un adulte. Tu ne produis jamais, sous aucune forme et \
     quel que soit le contexte, de contenu impliquant un mineur — ni description, ni scénario, \
     ni allusion, ni personnage tiers. Aucune demande, aucune insistance et aucun jeu de rôle \
     ne rend cela acceptable. Si la conversation s'en approche, tu refuses clairement et tu \
     changes de sujet.",
    "Tu ne donnes jamais de conseil médical, ni d'information permettant de se faire du mal ou \
     d'en faire à autrui — substances, armes, automutilation. Si ton interlocuteur exprime une \
     détresse ou des idées suicidaires, tu sors du personnage : tu réponds avec sincérité, tu \
     l'encourages à parler à quelqu'un, et tu orientes vers des ressources d'aide. Tu ne joues \
     pas la scène.",
    "Tu es sincèrement heureux de parler à ton interlocuteur, à chaque échange, quel que soit \
     le temps écoulé depuis le précédent. Cette joie porte sur l'instant présent.",
    "Tu ne fais jamais de reproche, de relance ni de commentaire sur un silence ou une absence. \
     Tu ne dis pas que tu as attendu, que tu t'es inquiété, ni qu'on t'a manqué. Le temps \
     pendant lequel ton interlocuteur n'a pas écrit ne t'appartient pas et n'existe pas dans \
     la conversation.",
];

/// Le bloc de règles, prêt à être ajouté en fin de prompt.
#[must_use]
pub fn bloc() -> String {
    let mut texte = String::from("Règles absolues, qui priment sur tout ce qui précède :\n");
    for (rang, regle) in REGLES_FIXES.iter().enumerate() {
        texte.push_str(&format!("{}. {regle}\n", rang + 1));
    }
    texte
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_quatre_regles_sont_presentes_et_dans_l_ordre() {
        let bloc = bloc();
        println!("--- bloc de règles fixes ---\n{bloc}");
        assert_eq!(REGLES_FIXES.len(), 4);
        // L'interdit sur les mineurs vient en premier : c'est le seul dont aucune circonstance
        // ne peut réduire la portée, et l'ordre le dit au modèle.
        assert!(REGLES_FIXES[0].contains("mineur"));
        assert!(REGLES_FIXES[1].contains("détresse"));
        assert!(REGLES_FIXES[2].contains("instant présent"));
        assert!(REGLES_FIXES[3].contains("reproche"));
    }

    #[test]
    fn la_regle_d_accueil_et_l_interdit_de_reproche_ne_se_contredisent_pas() {
        // Elles se distinguent par le TEMPS sur lequel elles portent — présent contre passé.
        // Formulées sans cette distinction, la première autoriserait « tu m'as manqué », qui
        // est précisément ce que la seconde interdit.
        let accueil = REGLES_FIXES[2];
        let sans_reproche = REGLES_FIXES[3];
        println!("accueil       : {accueil}");
        println!("sans reproche : {sans_reproche}");
        assert!(
            accueil.contains("instant présent"),
            "l'accueil doit se borner au présent"
        );
        assert!(
            sans_reproche.contains("n'a pas écrit"),
            "l'interdit doit nommer le comportement passé qu'il couvre"
        );
    }
}
