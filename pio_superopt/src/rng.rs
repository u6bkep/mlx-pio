//! Tiny deterministic PRNG (splitmix64). Seeded, reproducible, no deps —
//! every search run is replayable from its seed.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// The full internal state (splitmix64 is just its counter). Together
    /// with [`Rng::from_state`] this makes a search resumable mid-stream:
    /// restoring the state continues the exact draw sequence.
    pub fn state(&self) -> u64 {
        self.0
    }

    pub fn from_state(state: u64) -> Self {
        Rng(state)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n must be > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn boolean(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// A uniformly-chosen element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u32) as usize]
    }
}
