//! Small seeded generator used for noise permutations and Liquify jitter.
//!
//! Determinism per seed is required so that undo and redo replay identically (spec
//! section 19.5). SplitMix64 is used; the design notes do not fix a generator.

/// Deterministic SplitMix64 generator.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform value in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Uniform point in the unit disc.
    pub fn unit_disc(&mut self) -> [f32; 2] {
        loop {
            let x = 2.0 * self.next_f32() - 1.0;
            let y = 2.0 * self.next_f32() - 1.0;
            if x * x + y * y <= 1.0 {
                return [x, y];
            }
        }
    }
}
