//! Le temps, lu en un seul endroit.
//!
//! Regrouper la lecture ici n'est pas une coquetterie : le jour où un test devra figer l'heure
//! — « trois jours sans nouvelles », « il est deux heures du matin », toute la logique d'état
//! de relation de la phase 2 — il n'y aura qu'un point à détourner, et non un
//! `SystemTime::now()` dispersé dans quinze modules.
//!
//! Le module n'expose **que** l'horloge murale. Un wrapper monotone y a figuré un temps sans
//! aucun appelant : il annonçait une convention que le seul code mesurant des durées violait
//! déjà, et une fonction libre rendant un `Instant` n'offre de toute façon aucun point
//! d'injection — figer le temps demandera un trait ou un paramètre, pas un alias. Il
//! réapparaîtra avec son premier consommateur réel.

use std::time::{SystemTime, UNIX_EPOCH};

/// Secondes écoulées depuis l'époque Unix.
///
/// Un `i64` et non un `u64` : les durées se soustraient, et une soustraction d'instants doit
/// pouvoir être négative sans faire le tour du compteur.
///
/// Renvoie `0` si l'horloge système est antérieure à l'époque — cas qui ne se produit que sur
/// une machine dont l'horloge n'a jamais été réglée, et où échouer bruyamment n'aiderait
/// personne.
#[must_use]
pub fn maintenant() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
