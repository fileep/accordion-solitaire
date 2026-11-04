//! Dead-position table: a lossy set of 64-bit position hashes, 8 to a 64-byte bucket (RESEARCH.md §6.6).
//! An eviction or a miss only costs search time. A false hit could hide a win but never invent one, and every
//! win is verified by the reference engine before it counts.
//!
//! Each entry carries the table's current epoch in its low byte, so clearing between searches is O(1): bump
//! the epoch and every older entry stops matching and becomes free to overwrite. A real wipe happens only when
//! the 8-bit epoch wraps.

#[derive(Clone, Copy, Default)]
#[repr(C, align(64))]
struct Bucket([u64; 8]);

pub struct DeadTable {
    buckets: Vec<Bucket>,
    mask: usize,
    /// 1–255; 0 marks a never-used slot.
    epoch: u64,
}

impl DeadTable {
    /// A table of `2^log2_buckets` buckets (64 bytes each).
    pub fn new(log2_buckets: u32) -> DeadTable {
        let n = 1usize << log2_buckets;
        DeadTable { buckets: vec![Bucket::default(); n], mask: n - 1, epoch: 1 }
    }

    pub fn bytes(&self) -> usize {
        self.buckets.len() * 64
    }

    #[inline(always)]
    fn index(&self, hash: u64) -> usize {
        (hash >> 40) as usize & self.mask
    }

    /// The high 56 bits of the hash, tagged with the current epoch.
    #[inline(always)]
    fn key(&self, hash: u64) -> u64 {
        (hash & !0xFF) | self.epoch
    }

    #[inline]
    pub fn contains(&self, hash: u64) -> bool {
        self.buckets[self.index(hash)].0.contains(&self.key(hash))
    }

    #[inline]
    pub fn insert(&mut self, hash: u64) {
        let (key, epoch, i) = (self.key(hash), self.epoch, self.index(hash));
        let bucket = &mut self.buckets[i].0;
        match bucket.iter().position(|&e| e == key || e & 0xFF != epoch) {
            Some(slot) => bucket[slot] = key, // the same entry, or a free or stale slot
            None => bucket[(hash >> 1) as usize & 7] = key,
        }
    }

    /// Starts loading the bucket for `hash` so a later lookup doesn't wait on memory.
    #[inline(always)]
    pub fn prefetch(&self, hash: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            let ptr = &self.buckets[self.index(hash)] as *const Bucket as *const i8;
            // SAFETY: a prefetch is only a hint; the pointer is in bounds of `buckets` regardless.
            unsafe { core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(ptr) };
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = hash;
    }

    /// Forgets every entry in O(1) (a full wipe once every 255 clears).
    pub fn clear(&mut self) {
        self.epoch += 1;
        if self.epoch > 0xFF {
            self.buckets.fill(Bucket::default());
            self.epoch = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_what_fits_and_never_reports_absent_keys() {
        let mut t = DeadTable::new(4);
        let keys: Vec<u64> = (1..=100u64).map(|k| k.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        for &k in &keys[..60] {
            t.insert(k);
        }
        let kept = keys[..60].iter().filter(|&&k| t.contains(k)).count();
        assert!(kept >= 40, "{kept}"); // 16 buckets x 8 slots, some evicted
        assert!(keys[60..].iter().all(|&k| !t.contains(k)));
        t.clear();
        assert!(keys.iter().all(|&k| !t.contains(k)));
    }

    #[test]
    fn clearing_survives_epoch_wraparound() {
        let mut t = DeadTable::new(2);
        for round in 0..600u64 {
            let k = round.wrapping_mul(0xD1B5_4A32_D192_ED03);
            t.insert(k);
            assert!(t.contains(k));
            t.clear();
            assert!(!t.contains(k), "round {round}");
        }
    }
}
