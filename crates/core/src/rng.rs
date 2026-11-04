//! Deterministic randomness for deals: SplitMix64 seeds xoshiro256**, which drives a Fisher–Yates
//! shuffle. Specified exactly in docs/FORMATS.md so any implementation reproduces a deal from its seed.

/// SplitMix64 (Steele, Lea & Flood). Used only to expand a 64-bit seed into xoshiro256** state.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// xoshiro256** 1.0 (Blackman & Vigna).
pub struct Xoshiro256StarStar([u64; 4]);

impl Xoshiro256StarStar {
    /// State words are the first four SplitMix64 outputs for `seed`.
    pub fn from_seed(seed: u64) -> Xoshiro256StarStar {
        let mut sm = SplitMix64::new(seed);
        Xoshiro256StarStar(std::array::from_fn(|_| sm.next_u64()))
    }

    pub fn from_state(state: [u64; 4]) -> Xoshiro256StarStar {
        Xoshiro256StarStar(state)
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.0;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Uniform integer in `0..n` (n > 0) by Lemire's multiply-and-reject method: take the high 64 bits
    /// of `next * n`, redrawing while the low 64 bits fall below `2^64 mod n`.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        let mut m = u128::from(self.next_u64()) * u128::from(n);
        if (m as u64) < n {
            let threshold = n.wrapping_neg() % n;
            while (m as u64) < threshold {
                m = u128::from(self.next_u64()) * u128::from(n);
            }
        }
        (m >> 64) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_values() {
        // Published test vector for seed 1234567.
        let mut sm = SplitMix64::new(1_234_567);
        let expected = [
            6457827717110365317,
            3203168211198807973,
            9817491932198370423,
            4593380528125082431,
            16408922859458223821,
        ];
        for e in expected {
            assert_eq!(sm.next_u64(), e);
        }
    }

    #[test]
    fn xoshiro256starstar_reference_values() {
        // From state [1, 2, 3, 4], as in the reference C implementation.
        let mut x = Xoshiro256StarStar::from_state([1, 2, 3, 4]);
        for e in [11520, 0, 1509978240] {
            assert_eq!(x.next_u64(), e);
        }
    }

    #[test]
    fn below_stays_in_range_and_hits_every_value() {
        let mut x = Xoshiro256StarStar::from_seed(7);
        let mut seen = [false; 52];
        for _ in 0..10_000 {
            let v = x.below(52);
            assert!(v < 52);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(x.below(1), 0);
    }
}
