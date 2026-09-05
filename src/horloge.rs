//! Le temps, lu en un seul endroit.
//!
//! Regrouper les deux appels ici n'est pas une coquetterie : le jour où un test devra figer
//! l'heure — « trois jours sans nouvelles », « il est deux heures du matin », toute la logique
//! d'état de relation de la phase 2 — il n'y aura qu'un point à détourner, et non un
//! `SystemTime::now()` dispersé dans quinze modules.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// Instant monotone, pour mesurer des durées.
///
/// Distinct de [`maintenant`] : l'horloge murale peut reculer (NTP, changement d'heure), un
/// `Instant` non. Tout ce qui mesure « depuis combien de temps » utilise celui-ci.
#[must_use]
pub fn instant() -> Instant {
    Instant::now()
}
