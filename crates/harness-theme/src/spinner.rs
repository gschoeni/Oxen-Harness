//! The shared spinner *rhythm* — how a theme's glyphs and thinking phrases
//! rotate over time while the model works.
//!
//! The phrases and glyphs themselves live in [`crate::Voice`]; this module owns
//! the timing: one glyph per animation frame, a fresh phrase every
//! [`FRAMES_PER_PHRASE`] frames, starting from a seeded-random phrase so every
//! run opens on a different line. The CLI spinner drives [`Rhythm`] directly;
//! the desktop app mirrors the same constants in `app/src/lib/thinking.ts` so
//! both hosts breathe at the same cadence. Change it here, change it there.

/// Milliseconds per animation frame (one glyph step).
pub const FRAME_MS: u64 = 110;

/// A new phrase every this many frames (~1.8s at [`FRAME_MS`]).
pub const FRAMES_PER_PHRASE: u64 = 16;

/// One step of a xorshift64 PRNG — enough randomness to pick a starting
/// phrase without pulling in `rand`.
fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// The animation clock for one spinner: which glyph frame we're on and which
/// phrase is showing. Pure bookkeeping — no I/O, no timers — so any host
/// (CLI thread, async composer loop, webview) can drive it at [`FRAME_MS`].
#[derive(Clone, Debug)]
pub struct Rhythm {
    frame: u64,
    phrase_idx: usize,
    phrase_count: usize,
}

impl Rhythm {
    /// A rhythm over `phrase_count` phrases, opening on a seed-picked phrase.
    /// A zero/one-phrase pool is fine — the index just stays at 0.
    pub fn new(phrase_count: usize, seed: u64) -> Self {
        let mut s = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
        let phrase_idx = if phrase_count > 1 {
            (xorshift(&mut s) as usize) % phrase_count
        } else {
            0
        };
        Rhythm {
            frame: 0,
            phrase_idx,
            phrase_count,
        }
    }

    /// Advance one frame, rotating to the next phrase every
    /// [`FRAMES_PER_PHRASE`] frames.
    pub fn tick(&mut self) {
        self.frame += 1;
        if self.phrase_count > 0 && self.frame % FRAMES_PER_PHRASE == 0 {
            self.phrase_idx = (self.phrase_idx + 1) % self.phrase_count;
        }
    }

    /// Index of the phrase currently showing (0 when the pool is empty).
    pub fn phrase_index(&self) -> usize {
        self.phrase_idx
    }

    /// Index into a `glyph_count`-frame glyph cycle for the current frame.
    pub fn glyph_index(&self, glyph_count: usize) -> usize {
        if glyph_count == 0 {
            0
        } else {
            (self.frame as usize) % glyph_count
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_rotates_every_frames_per_phrase() {
        let mut r = Rhythm::new(3, 42);
        let start = r.phrase_index();
        for _ in 0..FRAMES_PER_PHRASE - 1 {
            r.tick();
            assert_eq!(r.phrase_index(), start, "phrase changed early");
        }
        r.tick();
        assert_eq!(r.phrase_index(), (start + 1) % 3);
        for _ in 0..FRAMES_PER_PHRASE {
            r.tick();
        }
        assert_eq!(r.phrase_index(), (start + 2) % 3);
    }

    #[test]
    fn glyphs_cycle_per_frame() {
        let mut r = Rhythm::new(5, 1);
        assert_eq!(r.glyph_index(4), 0);
        r.tick();
        assert_eq!(r.glyph_index(4), 1);
        for _ in 0..3 {
            r.tick();
        }
        assert_eq!(r.glyph_index(4), 0); // wrapped
        assert_eq!(r.glyph_index(0), 0); // empty glyph pool is safe
    }

    #[test]
    fn seed_varies_the_opening_phrase_and_empty_pools_are_safe() {
        // Different seeds should (for some pair) pick different openings.
        let picks: Vec<usize> = (1..=8u64)
            .map(|s| Rhythm::new(19, s * 0x1234_5678).phrase_index())
            .collect();
        assert!(picks.iter().any(|&p| p != picks[0]));
        assert!(picks.iter().all(|&p| p < 19));

        let mut empty = Rhythm::new(0, 7);
        empty.tick();
        assert_eq!(empty.phrase_index(), 0);

        // A zero seed must not lock the PRNG at zero forever.
        assert!(Rhythm::new(19, 0).phrase_index() < 19);
    }
}
