//! The trail "almanac": the small, `Ui`-free generators behind the Oregon Trail
//! flavor — today's date, a random weather reading, and a tiny time-seeded
//! xorshift PRNG used to pick a death line or spinner verb.
//!
//! Kept dependency-free on purpose: a civil-date conversion stands in for a
//! date/time crate, and the xorshift stands in for `rand`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Today's date formatted like the journal's flavor ("March 21, 1848"), but for
/// the present day. Derived from `SystemTime` (UTC) with a civil-date conversion
/// so we avoid pulling in a date/time dependency.
pub(crate) fn today() -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{} {}, {}", MONTHS[(month - 1) as usize], day, year)
}

/// Convert a count of days since the Unix epoch (1970-01-01) into a
/// (year, month, day) civil date. Algorithm from Howard Hinnant's `chrono`
/// date library (`civil_from_days`), valid across the Gregorian calendar.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A random weather reading for the trail journal — Oregon Trail flavor, picked
/// fresh each run from the same time-seeded PRNG used for the death screen.
pub(crate) fn weather() -> &'static str {
    const CONDITIONS: [&str; 10] = [
        "warm", "hot", "cool", "cold", "freezing", "rainy", "snowy", "foggy", "windy", "clear",
    ];
    let mut s = seed();
    CONDITIONS[(xorshift(&mut s) as usize) % CONDITIONS.len()]
}

/// A nonzero PRNG seed from the current time (falls back to a fixed odd
/// constant if the clock is unavailable).
pub(crate) fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
        | 1
}

/// One step of a xorshift64 PRNG, advancing `state` and returning the new value.
pub(crate) fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Pick a pseudo-random entry from `pool` (empty string if the pool is empty).
pub(crate) fn pick(pool: &[String]) -> &str {
    if pool.is_empty() {
        return "";
    }
    let mut s = seed();
    let idx = (xorshift(&mut s) as usize) % pool.len();
    &pool[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // Unix epoch
        assert_eq!(civil_from_days(18_993), (2022, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1)); // non-leap year
        assert_eq!(civil_from_days(-719_162), (1, 1, 1));
    }

    #[test]
    fn weather_is_one_of_the_known_conditions() {
        const CONDITIONS: [&str; 10] = [
            "warm", "hot", "cool", "cold", "freezing", "rainy", "snowy", "foggy", "windy", "clear",
        ];
        assert!(CONDITIONS.contains(&weather()));
    }

    #[test]
    fn pick_is_empty_for_empty_pool_and_in_bounds_otherwise() {
        assert_eq!(pick(&[]), "");
        let pool = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(pool.iter().any(|x| x == pick(&pool)));
    }
}
