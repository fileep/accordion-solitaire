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

/// ChaCha20 (RFC 8439) keyed by the seed, for dealing with a generator that shares nothing with the one above.
/// Only the `second-generator` feature uses it: see [`crate::Deal::from_seed`].
pub struct ChaCha20 {
    key: [u32; 8],
    counter: u32,
    block: [u32; 16],
    used: usize,
}

impl ChaCha20 {
    pub fn from_seed(seed: u64) -> ChaCha20 {
        let mut key = [0; 8];
        key[0] = seed as u32;
        key[1] = (seed >> 32) as u32;
        ChaCha20 { key, counter: 0, block: [0; 16], used: 64 }
    }

    /// The block function of RFC 8439, section 2.3.
    fn block(key: &[u32; 8], counter: u32, nonce: [u32; 3]) -> [u32; 16] {
        let mut start = [0; 16];
        start[..4].copy_from_slice(&[0x6170_7865, 0x3320_646E, 0x7962_2D32, 0x6B20_6574]);
        start[4..12].copy_from_slice(key);
        start[12] = counter;
        start[13..].copy_from_slice(&nonce);
        let mut s = start;
        let mut quarter = |a: usize, b: usize, c: usize, d: usize| {
            s[a] = s[a].wrapping_add(s[b]);
            s[d] = (s[d] ^ s[a]).rotate_left(16);
            s[c] = s[c].wrapping_add(s[d]);
            s[b] = (s[b] ^ s[c]).rotate_left(12);
            s[a] = s[a].wrapping_add(s[b]);
            s[d] = (s[d] ^ s[a]).rotate_left(8);
            s[c] = s[c].wrapping_add(s[d]);
            s[b] = (s[b] ^ s[c]).rotate_left(7);
        };
        for _ in 0..10 {
            quarter(0, 4, 8, 12);
            quarter(1, 5, 9, 13);
            quarter(2, 6, 10, 14);
            quarter(3, 7, 11, 15);
            quarter(0, 5, 10, 15);
            quarter(1, 6, 11, 12);
            quarter(2, 7, 8, 13);
            quarter(3, 4, 9, 14);
        }
        std::array::from_fn(|i| s[i].wrapping_add(start[i]))
    }

    fn next_byte(&mut self) -> u8 {
        if self.used == 64 {
            self.block = ChaCha20::block(&self.key, self.counter, [0; 3]);
            self.counter += 1;
            self.used = 0;
        }
        let byte = (self.block[self.used / 4] >> (8 * (self.used % 4))) as u8;
        self.used += 1;
        byte
    }

    /// Uniform integer in `0..n` (0 < n <= 256) by masking and redrawing, a different method from Lemire's above.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!((1..=256).contains(&n), "below({n})");
        let mask = (1u64 << (64 - (n - 1).leading_zeros())) - 1;
        loop {
            let v = u64::from(self.next_byte()) & mask;
            if v < n {
                return v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chacha20_rfc8439_block() {
        // The test vector of RFC 8439, section 2.3.2.
        let key = [
            0x0302_0100,
            0x0706_0504,
            0x0B0A_0908,
            0x0F0E_0D0C,
            0x1312_1110,
            0x1716_1514,
            0x1B1A_1918,
            0x1F1E_1D1C,
        ];
        let expected = [
            0xE4E7_F110,
            0x1559_3BD1,
            0x1FDD_0F50,
            0xC471_20A3,
            0xC7F4_D1C7,
            0x0368_C033,
            0x9AAA_2204,
            0x4E6C_D4C3,
            0x4664_82D2,
            0x09AA_9F07,
            0x05D7_C214,
            0xA202_8BD9,
            0xD19C_12B5,
            0xB94E_16DE,
            0xE883_D0CB,
            0x4E3C_50A2,
        ];
        assert_eq!(ChaCha20::block(&key, 1, [0x0900_0000, 0x4A00_0000, 0]), expected);
    }

    #[test]
    fn chacha20_below_stays_in_range_and_hits_every_value() {
        let mut x = ChaCha20::from_seed(7);
        for n in [1, 2, 3, 4, 5, 8, 52, 255, 256] {
            let mut seen = vec![false; n as usize];
            for _ in 0..20_000 {
                let v = x.below(n);
                assert!(v < n);
                seen[v as usize] = true;
            }
            assert!(seen.iter().all(|&s| s), "n = {n}");
        }
    }

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
