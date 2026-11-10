//! GPU ladder attempts (PLAN.md Phase 5 track F, Phase 8). The kernel (`kernel.cu`, compiled at run time with
//! NVRTC) runs one attempt per thread: a random prefix, then a small first-win search of what is left. The host
//! sends positions in batches short enough for Windows' GPU watchdog, and every win comes back as a move line for
//! the CPU to verify. A batch is submitted on a stream and collected later; [`Gpu::feed`] keeps three in flight, so
//! the GPU never waits for the host.
//!
//! `acc batch --gpu` runs this next to the CPU solver (crates/cli/src/runner.rs); `accgpu` measures it alone.

use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_core::quick;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DriverError, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;
use cudarc::driver::{CudaEvent, PinnedHostSlice};

const SOURCE: &str = include_str!("kernel.cu");

// FEED2 timing (off unless set): ACC_FEEDPROF=<file> appends a summary every 50 launches of Gpu::feed.
fn feedprof() -> Option<&'static str> {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var("ACC_FEEDPROF").ok()).as_deref()
}
struct Prof {
    take: f64,
    fill: f64,
    wait: f64,
    htod: f64,
    alloc: f64,
    launch: f64,
    before: u8,
    after: u8,
    ev: [CudaEvent; 4],
}
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
fn prev_state<T>(q: &VecDeque<(Vec<T>, Pending)>) -> u8 {
    match q.back() {
        None => 2,
        Some((_, p)) => match &p.prof {
            Some(pr) => u8::from(pr.ev[2].is_complete()),
            None => 9,
        },
    }
}
#[derive(Default)]
struct ProfSum {
    n: u32,
    v: [f64; 16],
    mx: [f64; 16],
    before: [u32; 3],
    after: [u32; 3],
    gaps: u32,
    prev_mid: Option<f64>,
}
impl ProfSum {
    fn add(&mut self, i: usize, x: f64) {
        self.v[i] += x;
        self.mx[i] = self.mx[i].max(x);
    }
}

/// Fills a position's bytes after its last pile.
pub const SENTINEL: u8 = 0x7F;
/// The kernel's limits: random moves before a search, and piles a search can hold.
pub const MAX_PREFIX: u32 = 48;
pub const RESIDUAL: usize = 16;
/// Most attempts per position over both rungs: the kernel counts them in the low 16 bits of the position's counter.
pub const MAX_TRIES: u32 = 0xFFFF;
/// NVRTC 12.9 writes PTX that needs a driver for CUDA 12.9 or later.
const MIN_DRIVER: i32 = 12090;
/// The kernel's profile counters (kernel.cu, with [`Settings::profile`]), in order: cycles of its prefix warps taking
/// work, loading the position, playing the prefix and queueing residuals, and of its search warps searching, taking
/// residuals and writing results, and waiting for residuals (summed over threads); steps of the search loop by warps
/// and by threads, and of the prefix loop; attempts taken and skipped; thread spans summed (ns); the latest end (ns);
/// the earliest start, bit-inverted (ns); and turns of prefix threads with a residual the queue had no room for, and
/// of search threads with none to take.
pub const PROFILE_SLOTS: usize = 18;

/// How `acc batch` uses the GPU (`--gpu-config`, JSON; fields left out keep these defaults, measured in Phase 5
/// track F).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// One or two rungs, as [random moves before the search, tries per deal]. Both run in one launch, the second
    /// on the deals the first didn't win. Deals need at least 36 random moves to leave the search ≤16 piles. Tries are
    /// numbered as if every group had `share_group` of them, though a deal's first `SOLO_GROUPS` groups (kernel.cu) have
    /// one: with groups of 2, the default's first rung is 12 tries of 40 moves, then 38 moves from try 24 on.
    pub rungs: Vec<[u32; 2]>,
    /// Positions each search may generate.
    pub budget: u32,
    /// Deals per launch: keep launches well under Windows' 2 s GPU watchdog (65,536 take about 0.1 s on the T550, and
    /// fewer leave several attempts at once on each deal, which wastes those still running once it is won). 262,144
    /// with a first rung of 16 ran hot x1.044 over 131,072 with 128 (docs/BENCHMARKS.md).
    pub batch: usize,
    /// Threads per block (the kernel's shared memory is sized for it: 64 bytes per thread). On the T550, 64 fits
    /// 1,024 attempts per multiprocessor, its maximum (32 fits 512, the most blocks it takes); the queue takes
    /// shared memory too, 8 blocks of 64 (one prefix warp, one search warp) fit, the default: 8 of 64 blocks
    /// ran 8.6 to 9% faster than 5 of 96, and 10 of 64 was no better (docs/BENCHMARKS.md, "After Phase 8 — the search
    /// step, exact choice, and what the rest came to").
    pub block: u32,
    /// Thread blocks per multiprocessor. Fewer than fit, when they keep it busy, leave fewer attempts running on deals
    /// already won.
    pub blocks_per_sm: u32,
    /// Connectivity test at positions with at most this many piles.
    pub conn_gate: u32,
    /// The stronger connectivity test that leaves pile 0's card out (accordion_core::fast's
    /// `connected_without_leftmost`): about 50% faster on the T550.
    pub leftmost: bool,
    /// How much likelier a prefix leap is than a slide, a power of two (1: every legal move alike; kernel.cu
    /// `prefix_move`).
    pub leap_weight: u32,
    pub seed: u64,
    /// Compile the kernel with its profile counters ([`BatchResult::profile`]); slower, for measuring only.
    pub profile: bool,
    /// The warps of a block that play prefixes (kernel.cu `PREFIX_THREADS`); `block` must hold more.
    pub prefix_warps: u32,
    /// Shared early prefix moves (kernel.cu `SHARE_G` and `SHARE_K`): a
    /// deal's tries go in groups of `share_group`, whose first `share_moves` prefix moves are played once; each try
    /// then plays the rest of its prefix from the row they left, and searches. 1 shares nothing.
    pub share_group: u32,
    /// From 20 (the masks a group keeps then fit 32 bits) up to the shorter rung's random moves when `share_group` is
    /// more than 1.
    pub share_moves: u32,
    /// The most ranks a residual of each rung may have to be searched (kernel.cu `RANK_GATE`).
    pub rank_gate: [u32; 2],
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            rungs: vec![[40, 24], [38, 376]],
            budget: 700,
            batch: 262144,
            block: 64,
            blocks_per_sm: 8,
            conn_gate: 19,
            leftmost: true,
            leap_weight: 8,
            seed: 0x5EED,
            profile: false,
            prefix_warps: 1,
            share_group: 2,
            share_moves: 20,
            rank_gate: [8, 10],
        }
    }
}

impl Settings {
    pub fn check(&self) -> Result<(), String> {
        if self.rungs.is_empty() || self.rungs.len() > 2 {
            return Err("rungs: one or two (both run in one launch)".into());
        }
        if let Some([p, _]) = self.rungs.iter().find(|[p, _]| !(36..=MAX_PREFIX).contains(p)) {
            return Err(format!(
                "rungs: {p} random moves; 36 to {MAX_PREFIX} (fewer leave more than {RESIDUAL} piles to search)"
            ));
        }
        let tries: u64 = self.rungs.iter().map(|&[_, t]| u64::from(t)).sum();
        if tries > u64::from(MAX_TRIES) {
            return Err(format!("rungs: {tries} tries per deal in all; at most {MAX_TRIES}"));
        }
        if !self.leap_weight.is_power_of_two() || self.leap_weight > 1 << 16 {
            return Err(format!("leap_weight: {}; a power of two up to 65536", self.leap_weight));
        }
        if self.batch == 0 || self.block == 0 || self.block > 256 || self.blocks_per_sm == 0 {
            return Err("batch, block (at most 256) and blocks_per_sm must be positive".into());
        }
        if !self.block.is_multiple_of(32) || self.prefix_warps == 0 || self.prefix_warps >= self.block / 32 {
            return Err(format!(
                "block {}, prefix_warps {}: whole warps, at least one playing prefixes and one searching",
                self.block, self.prefix_warps
            ));
        }
        if self.share_group == 0 || self.share_group > 64 {
            return Err(format!("share_group: {}; 1 to 64", self.share_group));
        }
        if let Some(shortest) = self.rungs.iter().map(|&[p, _]| p).min()
            && self.share_group > 1
            && !(20..=shortest).contains(&self.share_moves)
        {
            return Err(format!(
                "share_moves: {}; 20 to {shortest}, the shorter rung's moves",
                self.share_moves
            ));
        }
        Ok(())
    }

    /// The settings for launch number `launch`: its own random seed.
    pub fn attempts(&self, launch: u64) -> Attempts {
        let [prefix, tries] = self.rungs[0];
        let [prefix2, tries2] = self.rungs.get(1).copied().unwrap_or([0, 0]);
        Attempts {
            tries,
            prefix,
            tries2,
            prefix2,
            budget: self.budget,
            conn_gate: self.conn_gate,
            leftmost: self.leftmost,
            leap_shift: self.leap_weight.max(1).trailing_zeros(),
            seed: self.seed.wrapping_add(launch),
        }
    }
}

/// Settings for one launch: two rungs.
#[derive(Clone, Copy, Debug)]
pub struct Attempts {
    /// Attempts per position on the first rung, and random moves before each one's search.
    pub tries: u32,
    pub prefix: u32,
    /// The same for the second rung (0 tries: none).
    pub tries2: u32,
    pub prefix2: u32,
    /// Positions each search may generate.
    pub budget: u32,
    /// Connectivity test at positions with at most this many piles.
    pub conn_gate: u32,
    /// The stronger test without pile 0's card.
    pub leftmost: bool,
    /// A prefix leap is 2^leap_shift times as likely as a slide.
    pub leap_shift: u32,
    pub seed: u64,
}

/// What came back for a batch, as the kernel wrote it: nothing here takes work per position until it is asked for,
/// and many threads can ask at once.
pub struct BatchResult {
    /// Per position: not 0, won.
    won: Vec<u32>,
    /// Per position, 51 bytes: the winning line.
    lines: Vec<u8>,
    /// Per position: positions generated << 16 | attempts run.
    counts: Vec<u64>,
    /// Piles of each position: its line has one move fewer.
    piles: Vec<usize>,
    /// Each position's 52 bytes, as the GPU had them (empty for a batch it never got).
    codes: Vec<u8>,
    /// The kernel's [`PROFILE_SLOTS`] counters for the launch, when profiling (empty otherwise).
    pub profile: Vec<u64>,
    /// The masks that were uploaded, and where all these buffers go for the next launch once this is done with.
    masks: Vec<u64>,
    pool: Option<Pool>,
}

/// Buffers of a launch, to use again: at a launch every few tens of milliseconds, fresh ones cost thousands of page faults
/// a launch, on a CPU that may be short of power.
#[derive(Default)]
struct Spare {
    flat: Vec<u8>,
    masks: Vec<u64>,
    piles: Vec<usize>,
    won: Vec<u32>,
    lines: Vec<u8>,
    counts: Vec<u64>,
}

type Pool = Arc<Mutex<Vec<Spare>>>;

impl Drop for BatchResult {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            let spare = Spare {
                flat: std::mem::take(&mut self.codes),
                masks: std::mem::take(&mut self.masks),
                piles: std::mem::take(&mut self.piles),
                won: std::mem::take(&mut self.won),
                lines: std::mem::take(&mut self.lines),
                counts: std::mem::take(&mut self.counts),
            };
            pool.lock().expect("no panics").push(spare);
        }
    }
}

impl BatchResult {
    /// A batch of `n` positions the GPU never got to: none won.
    pub fn none(n: usize) -> BatchResult {
        BatchResult {
            won: vec![0; n],
            lines: Vec::new(),
            counts: vec![0; n],
            piles: vec![0; n],
            codes: Vec::new(),
            profile: Vec::new(),
            masks: Vec::new(),
            pool: None,
        }
    }

    pub fn len(&self) -> usize {
        self.won.len()
    }

    pub fn is_empty(&self) -> bool {
        self.won.is_empty()
    }

    pub fn is_won(&self, i: usize) -> bool {
        self.won[i] != 0
    }

    /// Position `i`'s winning line (compact moves, `pile << 1 | leap`), if an attempt won it.
    pub fn line(&self, i: usize) -> Option<&[u8]> {
        self.is_won(i).then(|| &self.lines[i * 51..i * 51 + self.piles[i] - 1])
    }

    /// Position `i`'s 52 bytes, as the GPU had them.
    pub fn codes(&self, i: usize) -> &[u8; 52] {
        self.codes[i * 52..(i + 1) * 52].try_into().expect("52 bytes")
    }

    /// Positions generated on position `i`, prefix moves included, over every attempt.
    pub fn nodes(&self, i: usize) -> u64 {
        self.counts[i] >> 16
    }

    /// Attempts run on position `i` (its later attempts are skipped once one has won it).
    pub fn tries(&self, i: usize) -> u32 {
        (self.counts[i] & 0xFFFF) as u32
    }

    /// [`BatchResult::nodes`] over the batch.
    pub fn total_nodes(&self) -> u64 {
        self.counts.iter().map(|&c| c >> 16).sum()
    }

    /// [`BatchResult::tries`] over the batch.
    pub fn total_tries(&self) -> u64 {
        self.counts.iter().map(|&c| c & 0xFFFF).sum()
    }
}

/// A launched batch, until its results are copied back.
pub struct Pending {
    stream: Arc<CudaStream>,
    /// Piles of each position: its line has one move fewer.
    piles: Vec<usize>,
    codes: Vec<u8>,
    host_masks: Vec<u64>,
    spare: (Vec<u32>, Vec<u8>, Vec<u64>),
    pool: Option<Pool>,
    /// The launch's buffers; its results come back into the slot's page-locked buffers on the stream.
    slot: Box<Slot>,
    profile: bool,
    prof: Option<Box<Prof>>,
}

/// One launch's device buffers and page-locked host buffers, kept from launch to launch by [`Gpu::feed`] (a slot a
/// stream). Pageable copies took about 43 ms (up) and 39 ms (down) of host time a launch against a 49 ms GPU cycle,
/// with the upload on the one thread that launches: the CPU's clock set the GPU's pace.
struct Slot {
    n: usize,
    /// Write-combined: the CPU only writes these.
    up_flat: PinnedHostSlice<u8>,
    up_masks: PinnedHostSlice<u64>,
    positions: CudaSlice<u8>,
    masks: CudaSlice<u64>,
    next: CudaSlice<u32>,
    next_line: CudaSlice<u32>,
    won: CudaSlice<u32>,
    lines: CudaSlice<u8>,
    /// Per position: positions generated << 16 | attempts run.
    counts: CudaSlice<u64>,
    profile: CudaSlice<u64>,
    down_won: PinnedHostSlice<u32>,
    down_lines: PinnedHostSlice<u8>,
    down_counts: PinnedHostSlice<u64>,
}

impl Slot {
    fn new(ctx: &Arc<CudaContext>, s: &Arc<CudaStream>, n: usize) -> Result<Slot, DriverError> {
        // SAFETY: every buffer is written (zeroed or copied into) on the stream before it is read.
        unsafe {
            Ok(Slot {
                n,
                up_flat: ctx.alloc_pinned(n * 52)?,
                up_masks: ctx.alloc_pinned(n * 3)?,
                positions: s.alloc(n * 52)?,
                masks: s.alloc(n * 3)?,
                next: s.alloc(1)?,
                next_line: s.alloc(1)?,
                won: s.alloc(n)?,
                lines: s.alloc(n * 51)?,
                counts: s.alloc(n)?,
                profile: s.alloc(PROFILE_SLOTS)?,
                down_won: ctx.alloc_pinned_with_flags(n, 0)?,
                down_lines: ctx.alloc_pinned_with_flags(n * 51, 0)?,
                down_counts: ctx.alloc_pinned_with_flags(n, 0)?,
            })
        }
    }
}

/// What [`Gpu::playouts`] recorded for one playout, step by step (kernel.cu `playouts`).
pub struct Playout {
    /// The position's slide and leap masks (bit i: pile i may move).
    pub masks: Vec<(u64, u64)>,
    /// The move played from it, `pile << 1 | leap`; the last step has none.
    pub moves: Vec<u8>,
    /// Bit 0: connected; bit 1: the move before broke a link; bit 2: undo failed; bit 3: connected without the
    /// leftmost card; bit 4: the register row (`attempts_split`'s prefix) disagreed with the row in shared memory.
    pub flags: Vec<u8>,
}

pub struct Gpu {
    ctx: Arc<CudaContext>,
    /// `attempts_split`.
    attempts: CudaFunction,
    write_lines: CudaFunction,
    playouts: CudaFunction,
    pub name: String,
    pub sms: u32,
    pub settings: Settings,
}

/// A driver error as the driver words it, with its name: `out of memory (CUDA_ERROR_OUT_OF_MEMORY)`.
fn err(e: DriverError) -> String {
    match (e.error_string(), e.error_name()) {
        (Ok(text), Ok(name)) => format!("{} ({})", text.to_string_lossy(), name.to_string_lossy()),
        _ => format!("CUDA error {:?}", e.0),
    }
}

/// A deal's position for the kernel.
pub fn deal_codes(deal: &Deal) -> [u8; 52] {
    let mut codes = [0u8; 52];
    for (c, card) in codes.iter_mut().zip(deal.cards()) {
        *c = card.code();
    }
    codes
}

/// A position of at most 52 piles (engine codes) for the kernel.
pub fn position_codes(codes: &[u8]) -> [u8; 52] {
    let mut out = [SENTINEL; 52];
    out[..codes.len()].copy_from_slice(codes);
    out
}

fn piles(position: &[u8; 52]) -> usize {
    // Nearly always all 52 piles, which this settles in a few instructions.
    if !position.iter().fold(false, |any, &c| any | (c == SENTINEL)) {
        return 52;
    }
    position.iter().position(|&c| c == SENTINEL).unwrap_or(52)
}

/// A batch of positions as the kernels take them.
struct Prepared {
    /// Each position's 52 bytes.
    flat: Vec<u8>,
    /// Each position's slide and leap masks and cards present (worked out once here, where a deal is tried many times
    /// there).
    masks: Vec<u64>,
    piles: Vec<usize>,
    /// The other buffers of a launch, and where they all go back to.
    spare: (Vec<u32>, Vec<u8>, Vec<u64>),
    pool: Option<Pool>,
}

/// Threads preparing a batch for [`Gpu::submit`].
const PREPARE_THREADS: usize = 4;
/// Threads preparing batches ahead for [`Gpu::feed`], a batch each, on their own.
const PREPARERS: usize = 3;
/// Launches [`Gpu::feed`] keeps on the GPU, each on a stream of its own, so that one starts as the last one's threads run out.
const IN_FLIGHT: usize = 3;

impl Prepared {
    fn new<T: Sync>(items: &[T], position: &(impl Fn(&T) -> [u8; 52] + Sync)) -> Prepared {
        let n = items.len();
        let mut p = Prepared {
            flat: vec![0; n * 52],
            masks: vec![0; n * 3],
            piles: vec![0; n],
            spare: Default::default(),
            pool: None,
        };
        let chunk = n.div_ceil(PREPARE_THREADS).max(1);
        std::thread::scope(|scope| {
            let parts = items
                .chunks(chunk)
                .zip(p.flat.chunks_mut(chunk * 52))
                .zip(p.masks.chunks_mut(chunk * 3))
                .zip(p.piles.chunks_mut(chunk));
            for (((items, flat), masks), piles_out) in parts {
                scope.spawn(move || {
                    for (i, item) in items.iter().enumerate() {
                        let codes = position(item);
                        let n = piles(&codes);
                        let (l1, l3, present) = if n == 52 { quick::masks52(&codes) } else { Position::masks_of_codes(&codes[..n]) };
                        flat[i * 52..(i + 1) * 52].copy_from_slice(&codes);
                        masks[i * 3..(i + 1) * 3].copy_from_slice(&[l1, l3, present]);
                        piles_out[i] = n;
                    }
                });
            }
        });
        p
    }

    /// On this thread alone, in buffers from `pool`.
    fn fill<T>(pool: &Pool, items: &[T], position: &impl Fn(&T) -> [u8; 52]) -> Prepared {
        let n = items.len();
        let spare = pool.lock().expect("no panics").pop().unwrap_or_default();
        let (mut flat, mut masks, mut piles_out) = (spare.flat, spare.masks, spare.piles);
        flat.resize(n * 52, 0);
        masks.resize(n * 3, 0);
        piles_out.resize(n, 0);
        let parts = flat.as_chunks_mut::<52>().0.iter_mut().zip(masks.as_chunks_mut::<3>().0.iter_mut()).zip(piles_out.iter_mut());
        for (item, ((flat, masks), piles_out)) in items.iter().zip(parts) {
            let codes = position(item);
            let n = piles(&codes);
            let (l1, l3, present) = if n == 52 { quick::masks52(&codes) } else { Position::masks_of_codes(&codes[..n]) };
            *flat = codes;
            *masks = [l1, l3, present];
            *piles_out = n;
        }
        Prepared {
            flat,
            masks,
            piles: piles_out,
            spare: (spare.won, spare.lines, spare.counts),
            pool: Some(Arc::clone(pool)),
        }
    }
}

/// ceil(2^64 / n), for the kernels to divide attempt numbers by n with a multiply (unused for n = 1).
fn reciprocal(n: usize) -> u64 {
    if n <= 1 { 0 } else { (u64::MAX / n as u64) + 1 }
}

/// What [`diagnose`] found of what the GPU needs. Driver versions are CUDA's, 12090 for 12.9.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Libraries {
    /// No NVIDIA driver: no NVIDIA card, or none installed for it.
    NoDriver,
    /// A driver older than the kernel needs.
    OldDriver(i32),
    /// A driver that will do, but no NVRTC, the compiler the kernel is built with when a run starts.
    NoNvrtc(i32),
    /// Both, as far as can be told without opening the GPU.
    Found(i32),
}

impl Libraries {
    /// A driver version as `12.9`.
    pub fn cuda(version: i32) -> String {
        format!("{}.{}", version / 1000, version % 1000 / 10)
    }
}

/// What opening the GPU says when NVRTC is missing. `acc batch --gpu auto` adds that it carries on on the CPU.
pub const NO_NVRTC: &str = "Your NVIDIA card could be used, but one free component (NVRTC) is missing. Run acc doctor \
                            for how to add it.";

/// Looks for the NVIDIA driver and NVRTC as opening the GPU does first, adding a copy of NVRTC ([`bundled_nvrtc`]) to
/// PATH on Windows when NVRTC isn't on it. It opens no CUDA context and runs nothing on the GPU, so it is safe beside
/// a run using it.
pub fn diagnose() -> Result<Libraries, String> {
    // SAFETY: loads and unloads the NVIDIA driver library, which cudarc would load anyway.
    if !unsafe { cudarc::driver::sys::is_culib_present() } {
        return Ok(Libraries::NoDriver);
    }
    let mut version = 0;
    // SAFETY: cuDriverGetVersion only writes the version; it needs no initialization.
    unsafe { cudarc::driver::sys::cuDriverGetVersion(&mut version) }.result().map_err(err)?;
    if version < MIN_DRIVER {
        return Ok(Libraries::OldDriver(version));
    }
    // SAFETY: as above, for NVRTC.
    let nvrtc = || unsafe { cudarc::nvrtc::sys::is_culib_present() };
    if !nvrtc() {
        add_bundled_nvrtc();
        if !nvrtc() {
            return Ok(Libraries::NoNvrtc(version));
        }
    }
    Ok(Libraries::Found(version))
}

/// Finds the NVIDIA driver and NVRTC without letting cudarc panic when one is missing ([`diagnose`]).
fn find_libraries() -> Result<(), String> {
    match diagnose()? {
        Libraries::NoDriver => Err("no NVIDIA driver found".into()),
        Libraries::OldDriver(version) => Err(format!(
            "the NVIDIA driver supports CUDA {}; the kernel needs 12.9 or later",
            Libraries::cuda(version)
        )),
        Libraries::NoNvrtc(_) => Err(NO_NVRTC.into()),
        Libraries::Found(_) => Ok(()),
    }
}

/// A copy of NVRTC where the system doesn't look: the folder `ACC_NVRTC_DIR` names (the one holding the library, or
/// one a wheel was unpacked into), or else the newest `.tools/cuda-nvrtc-*/nvidia/cuda_nvrtc/bin` (`lib` off Windows)
/// in the current directory or the executable's, or above them, where `tools/get-nvrtc` puts it.
#[cfg(any(windows, target_os = "linux"))]
fn bundled_nvrtc() -> Option<std::path::PathBuf> {
    let inner = if cfg!(windows) { "nvidia/cuda_nvrtc/bin" } else { "nvidia/cuda_nvrtc/lib" };
    let named = std::env::var_os("ACC_NVRTC_DIR").map(std::path::PathBuf::from).filter(|dir| dir.is_dir());
    if let Some(dir) = named {
        let unpacked = dir.join(inner);
        return Some(if unpacked.is_dir() { unpacked } else { dir });
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    starts.iter().flatten().flat_map(|p| p.ancestors()).find_map(|dir| {
        let mut copies: Vec<_> = std::fs::read_dir(dir.join(".tools"))
            .ok()?
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("cuda-nvrtc"))
            .map(|e| e.path().join(inner))
            .filter(|bin| bin.is_dir())
            .collect();
        copies.sort();
        copies.pop()
    })
}

/// Puts [`bundled_nvrtc`] first on PATH, which Windows reads each time a library is loaded.
#[cfg(windows)]
fn add_bundled_nvrtc() {
    if let Some(bin) = bundled_nvrtc() {
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = std::env::join_paths(paths) {
            // SAFETY: changing the environment is always safe on Windows (see std::env::set_var).
            unsafe { std::env::set_var("PATH", path) };
        }
    }
}

/// Elsewhere the loader reads its path only when a program starts: see [`restart_with_bundled_nvrtc`].
#[cfg(not(windows))]
fn add_bundled_nvrtc() {}

/// On Linux, starts this program again, once, with [`bundled_nvrtc`] first on `LD_LIBRARY_PATH`, when NVRTC can't be
/// loaded as things stand: the loader reads that variable only when a program starts. Call it first thing in `main`,
/// before anything is written, as it replaces the process (the same process ID, arguments and output). It does nothing
/// when there is no such copy, when it has started the program again already (`ACC_NVRTC_RESTARTED`), and off Linux.
pub fn restart_with_bundled_nvrtc() {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        const AGAIN: &str = "ACC_NVRTC_RESTARTED";
        if std::env::var_os(AGAIN).is_some() {
            return;
        }
        let Some(dir) = bundled_nvrtc() else { return };
        let old = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
        if std::env::split_paths(&old).any(|p| p == dir) {
            return;
        }
        // SAFETY: loads and unloads NVRTC if it can be found, as `diagnose` does.
        if unsafe { cudarc::nvrtc::sys::is_culib_present() } {
            return;
        }
        // An empty entry would mean the current directory.
        let kept = std::env::split_paths(&old).filter(|p| !p.as_os_str().is_empty());
        let (Ok(path), Ok(exe)) =
            (std::env::join_paths(std::iter::once(dir.clone()).chain(kept)), std::env::current_exe())
        else {
            return;
        };
        let mut args = std::env::args_os();
        let mut command = std::process::Command::new(exe);
        if let Some(name) = args.next() {
            command.arg0(name);
        }
        let e = command.args(args).env("LD_LIBRARY_PATH", path).env(AGAIN, "1").exec();
        eprintln!("note: couldn't start again with NVRTC from {} on LD_LIBRARY_PATH: {e}", dir.display());
    }
}

/// The built-in kernel's source (kernel.cu), for tools that look at the source a run would compile.
pub fn builtin_source() -> &'static str {
    SOURCE
}

/// Refuses a define that couldn't do what it says: a name the source never mentions, a name the kernel sets without
/// `#ifndef` (its own `#define` would win), or a name the host sets from [`Settings`] (the two would disagree).
pub fn check_defines(source: &str, defines: &[String]) -> Result<(), String> {
    const FROM_SETTINGS: [&str; 9] = [
        "BLOCK", "PREFIX_THREADS", "SHARE_G", "SHARE_K", "RANK_GATE", "RANK_GATE2", "LEFTMOST", "CONN_GATE", "PROFILE",
    ];
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for define in defines {
        let name = define.split('=').next().unwrap_or_default();
        if name.is_empty() || !name.chars().all(word) {
            return Err(format!("--define {define:?}: expected NAME=VALUE"));
        }
        if FROM_SETTINGS.contains(&name) {
            return Err(format!("--define {name}: the host sets this from the GPU settings; use --gpu-config"));
        }
        if !source.split(|c: char| !word(c)).any(|w| w == name) {
            return Err(format!("--define {name}: the kernel source never mentions it, so it would do nothing"));
        }
        let mut guarded = false;
        for line in source.lines() {
            let is_directive = line.trim_start().starts_with('#');
            let mut parts = line.trim_start().trim_start_matches('#').split_whitespace();
            let (first, second) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            if is_directive && second == name && (first == "ifndef" || first == "undef") {
                guarded = true;
            }
            if is_directive && first == "define" && second.split('(').next() == Some(name) && !guarded {
                return Err(format!(
                    "--define {name}: the kernel has its own #define {name} with no #ifndef, which would win"
                ));
            }
        }
    }
    Ok(())
}

/// The kernels (`source`: kernel.cu) compiled to PTX for `ctx`'s device with `settings` (its block size, shared
/// moves, and profiling).
fn compile(
    ctx: &CudaContext,
    settings: &Settings,
    source: &str,
    defines: &[String],
) -> Result<cudarc::nvrtc::Ptx, String> {
    use cudarc::driver::sys::CUdevice_attribute as A;
    let major = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR).map_err(err)?;
    let minor = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR).map_err(err)?;
    let arch: &'static str = Box::leak(format!("compute_{major}{minor}").into_boxed_str());
    compile_ptx_with_opts(
        source,
        CompileOptions {
            arch: Some(arch),
            options: [
                format!("-DBLOCK={}", settings.block),
                format!("-DPREFIX_THREADS={}", 32 * settings.prefix_warps),
                format!("-DSHARE_G={}", settings.share_group),
                format!("-DSHARE_K={}", if settings.share_group > 1 { settings.share_moves } else { 0 }),
                format!("-DRANK_GATE={}", settings.rank_gate[0]),
                format!("-DRANK_GATE2={}", settings.rank_gate[1]),
                format!("-DLEFTMOST={}", u32::from(settings.leftmost)),
                format!("-DCONN_GATE={}", settings.conn_gate),
            ]
            .into_iter()
            .chain(settings.profile.then(|| "-DPROFILE".to_string()))
            .chain(defines.iter().map(|d| format!("-D{d}")))
            .collect(),
            ..Default::default()
        },
    )
    .map_err(|e| compile_error(e, arch))
}

/// What NVRTC said when the kernel didn't compile, in its own words where it has them.
fn compile_error(e: cudarc::nvrtc::CompileError, arch: &str) -> String {
    match e {
        cudarc::nvrtc::CompileError::CompileError { log, .. } => {
            let log = log.to_string_lossy();
            if log.contains("gpu-architecture") {
                format!(
                    "the NVRTC found doesn't know this NVIDIA card ({arch}), which is newer than it: a newer CUDA 12 \
                     NVRTC may. NVRTC said: {}",
                    log.trim()
                )
            } else {
                format!("the kernel didn't compile. NVRTC said:\n{}", log.trim())
            }
        }
        other => format!("compiling the kernel, NVRTC failed: {other:?}"),
    }
}

impl Gpu {
    /// Compiles the kernels for device 0 with `settings` (its block size), or says why there is no usable GPU.
    pub fn new(settings: Settings) -> Result<Gpu, String> {
        Gpu::with_source(settings, SOURCE)
    }

    /// [`Gpu::new`] with another version of kernel.cu (`accgpu --kernel`, for trying changes without a rebuild).
    pub fn with_source(settings: Settings, source: &str) -> Result<Gpu, String> {
        Gpu::with_defines(settings, source, &[])
    }

    /// [`Gpu::with_source`] with `NAME=VALUE` defines for the compiler (`accgpu --define`). See [`check_defines`].
    pub fn with_defines(settings: Settings, source: &str, defines: &[String]) -> Result<Gpu, String> {
        use cudarc::driver::sys::CUdevice_attribute as A;
        settings.check()?;
        find_libraries()?;
        let ctx = CudaContext::new(0).map_err(|e| format!("no CUDA device: {}", err(e)))?;
        // Wait for the GPU by sleeping, not spinning, so the CPU core stays free for the CPU solver.
        ctx.set_blocking_synchronize().map_err(err)?;
        let name = ctx.name().unwrap_or_default();
        let sms = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT).map_err(err)? as u32;
        check_defines(source, defines)?;
        let ptx = compile(&ctx, &settings, source, defines)?;
        let module = ctx.load_module(ptx).map_err(|e| format!("loading the kernel: {}", err(e)))?;
        let attempts = module.load_function("attempts_split").map_err(err)?;
        let write_lines = module.load_function("write_lines").map_err(err)?;
        let playouts = module.load_function("playouts").map_err(err)?;
        Ok(Gpu { ctx, attempts, write_lines, playouts, name, sms, settings })
    }

    /// Thread blocks per launch.
    pub fn blocks(&self) -> u32 {
        self.sms * self.settings.blocks_per_sm
    }

    /// The kernels' PTX for these settings, as NVRTC writes it (to read what the code compiles to).
    pub fn ptx(settings: &Settings, source: Option<&str>) -> Result<String, String> {
        Gpu::ptx_with_defines(settings, source, &[])
    }

    /// [`Gpu::ptx`] with `NAME=VALUE` defines for the compiler.
    pub fn ptx_with_defines(
        settings: &Settings,
        source: Option<&str>,
        defines: &[String],
    ) -> Result<String, String> {
        check_defines(source.unwrap_or(SOURCE), defines)?;
        find_libraries()?;
        let ctx = CudaContext::new(0).map_err(|e| format!("no CUDA device: {}", err(e)))?;
        Ok(compile(&ctx, settings, source.unwrap_or(SOURCE), defines)?.to_src())
    }

    /// The attempt kernel's registers and local memory per thread, and how many of its blocks a multiprocessor holds.
    pub fn occupancy(&self) -> Result<(i32, i32, u32), String> {
        let f = &self.attempts;
        let blocks = f.occupancy_max_active_blocks_per_multiprocessor(self.settings.block, 0, None);
        Ok((f.num_regs().map_err(err)?, f.local_size_bytes().map_err(err)?, blocks.map_err(err)?))
    }

    /// Threads per launch, all resident at once when the multiprocessors hold that many blocks.
    pub fn threads(&self) -> u64 {
        u64::from(self.blocks()) * u64::from(self.settings.block)
    }

    /// A new stream to submit batches on.
    pub fn stream(&self) -> Result<Arc<CudaStream>, String> {
        self.ctx.new_stream().map_err(err)
    }

    /// Launches attempts on each position and returns at once; [`Gpu::blocks`] blocks of persistent threads share
    /// the work, and a position's remaining attempts are skipped once one has won it.
    pub fn submit(
        &self,
        stream: &Arc<CudaStream>,
        positions: &[[u8; 52]],
        a: &Attempts,
    ) -> Result<Pending, String> {
        self.submit_prepared(stream, Prepared::new(positions, &|p: &[u8; 52]| *p), a, None)
    }

    fn submit_prepared(
        &self,
        stream: &Arc<CudaStream>,
        prepared: Prepared,
        a: &Attempts,
        slot: Option<Box<Slot>>,
    ) -> Result<Pending, String> {
        if a.tries.saturating_add(a.tries2) > MAX_TRIES {
            return Err(format!(
                "{} tries per position in all; at most {MAX_TRIES}",
                a.tries.saturating_add(a.tries2)
            ));
        }
        let count = prepared.piles.len();
        let n = count as u32;
        let s = stream;
        let on = feedprof().is_some();
        let ev = |s: &Arc<CudaStream>| {
            if on { s.record_event(Some(cudarc::driver::sys::CUevent_flags::CU_EVENT_DEFAULT)).ok() } else { None }
        };
        let ta = Instant::now();
        let mut slot = match slot {
            Some(slot) if slot.n == count => slot,
            _ => Box::new(Slot::new(&self.ctx, s, count).map_err(err)?),
        };
        let alloc = ms(ta);
        let th = Instant::now();
        let e_h = ev(s);
        let Slot { up_flat, up_masks, positions, masks, next, next_line, won, lines, counts, profile, .. } =
            &mut *slot;
        up_flat.as_mut_slice().map_err(err)?.copy_from_slice(&prepared.flat);
        up_masks.as_mut_slice().map_err(err)?.copy_from_slice(&prepared.masks);
        s.memcpy_htod(up_flat, positions).map_err(err)?;
        s.memcpy_htod(up_masks, masks).map_err(err)?;
        for z in [s.memset_zeros(next), s.memset_zeros(next_line), s.memset_zeros(won), s.memset_zeros(lines)] {
            z.map_err(err)?;
        }
        s.memset_zeros(counts).map_err(err)?;
        s.memset_zeros(profile).map_err(err)?;
        let htod = ms(th);
        let tl = Instant::now();
        let e_s = ev(s);
        let inv = reciprocal(count);
        let leftmost = u32::from(a.leftmost);
        let mut launch = s.launch_builder(&self.attempts);
        launch
            .arg(&*positions)
            .arg(&*masks)
            .arg(&n)
            .arg(&inv)
            .arg(&a.tries)
            .arg(&a.prefix)
            .arg(&a.tries2)
            .arg(&a.prefix2)
            .arg(&a.budget)
            .arg(&a.conn_gate)
            .arg(&leftmost)
            .arg(&a.leap_shift)
            .arg(&a.seed)
            .arg(&mut *next)
            .arg(&mut *won)
            .arg(&mut *lines)
            .arg(&mut *counts)
            .arg(&mut *profile);
        let cfg = LaunchConfig {
            grid_dim: (self.blocks(), 1, 1),
            block_dim: (self.settings.block, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the argument list matches the kernel's signature in kernel.cu, and every buffer is sized for
        // `n` positions as the kernel indexes them.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        let e_m = ev(s);
        // On the same stream, so after the attempts are done.
        let mut launch = s.launch_builder(&self.write_lines);
        launch
            .arg(&*positions)
            .arg(&*masks)
            .arg(&n)
            .arg(&a.tries)
            .arg(&a.prefix)
            .arg(&a.tries2)
            .arg(&a.prefix2)
            .arg(&a.leap_shift)
            .arg(&a.seed)
            .arg(&*won)
            .arg(&mut *lines)
            .arg(&mut *next_line);
        let cfg = LaunchConfig { grid_dim: (self.blocks(), 1, 1), block_dim: (32, 1, 1), shared_mem_bytes: 0 };
        // SAFETY: the argument list matches write_lines's signature in kernel.cu, and every buffer is sized for `n`
        // positions as the kernel indexes them.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        let e_e = ev(s);
        let launch_ms = ms(tl);
        let prof = match (e_h, e_s, e_m, e_e) {
            (Some(h), Some(st), Some(m), Some(e)) => Some(Box::new(Prof {
                take: 0.0,
                fill: 0.0,
                wait: 0.0,
                htod,
                alloc,
                launch: launch_ms,
                before: 0,
                after: 0,
                ev: [h, st, m, e],
            })),
            _ => None,
        };
        Ok(Pending {
            prof,
            stream: s.clone(),
            piles: prepared.piles,
            codes: prepared.flat,
            host_masks: prepared.masks,
            spare: prepared.spare,
            pool: prepared.pool,
            slot,
            profile: self.settings.profile,
        })
    }

    /// [`Gpu::submit`] and wait for the result.
    pub fn run(&self, positions: &[[u8; 52]], a: &Attempts) -> Result<BatchResult, String> {
        let stream = self.ctx.default_stream();
        self.submit(&stream, positions, a)?.collect()
    }

    /// Runs the settings' rungs on batches of items from `next_batch` (`position` gives each one's bytes), and hands every
    /// finished batch to `done`, in order. Long-lived threads each prepare a whole batch ahead of time and launch it when its
    /// turn comes, so a launch is waiting when the one before ends, whatever else the CPU is doing: on a hot laptop, where
    /// the CPU gets 12 W beside the GPU, a batch prepared only once the last one had come back left the GPU idle up to a
    /// quarter of the time. This thread collects the results. On a GPU error, returns it with the items whose results never
    /// came back.
    pub fn feed<T: Sync + Send>(
        &self,
        position: impl Fn(&T) -> [u8; 52] + Sync,
        next_batch: impl FnMut() -> Option<Vec<T>> + Send,
        mut done: impl FnMut(Vec<T>, BatchResult),
    ) -> Result<(), (String, Vec<T>)> {
        struct Taking<F> {
            take: F,
            turn: u64,
        }
        struct State<T> {
            in_flight: VecDeque<(Vec<T>, Pending)>,
            /// The next turn to launch, and launches not yet known to be done.
            launch: u64,
            running: usize,
            preparers: usize,
            failed: Option<String>,
            lost: Vec<T>,
            /// Each stream's buffers, while no launch holds them.
            slots: Vec<Option<Box<Slot>>>,
        }
        let mut streams = Vec::with_capacity(IN_FLIGHT);
        for _ in 0..IN_FLIGHT {
            streams.push(self.stream().map_err(|e| (e, Vec::new()))?);
        }
        let taking = Mutex::new(Taking { take: next_batch, turn: 0 });
        let state = Mutex::new(State {
            in_flight: VecDeque::new(),
            launch: 0,
            running: 0,
            preparers: PREPARERS,
            failed: None,
            lost: Vec::new(),
            slots: (0..IN_FLIGHT).map(|_| None).collect(),
        });
        let changed = Condvar::new();
        let pool: Pool = Arc::default();
        let base = if feedprof().is_some() {
            streams[0].record_event(Some(cudarc::driver::sys::CUevent_flags::CU_EVENT_DEFAULT)).ok()
        } else {
            None
        };
        let base_t = Instant::now();
        let mut psum = ProfSum::default();
        let mut collected_turns = 0usize;
        let (position, streams) = (&position, &streams);
        std::thread::scope(|scope| {
            for _ in 0..PREPARERS {
                scope.spawn(|| {
                    loop {
                        let mut take_ms = 0.0;
                        let taken = {
                            let mut t = taking.lock().expect("no panics");
                            if state.lock().expect("no panics").failed.is_some() {
                                None
                            } else {
                                let tt = Instant::now();
                                let r = (t.take)().map(|items| {
                                    t.turn += 1;
                                    (t.turn - 1, items)
                                });
                                take_ms = ms(tt);
                                r
                            }
                        };
                        let Some((turn, items)) = taken else { break };
                        let tf = Instant::now();
                        let prepared = Prepared::fill(&pool, &items, position);
                        let fill_ms = ms(tf);
                        let tw = Instant::now();
                        let mut st = state.lock().expect("no panics");
                        while (st.launch != turn || st.running >= IN_FLIGHT) && st.failed.is_none() {
                            st = changed.wait(st).expect("no panics");
                        }
                        if st.failed.is_some() {
                            st.lost.extend(items);
                            break;
                        }
                        st.running += 1;
                        let before = prev_state(&st.in_flight);
                        let slot = st.slots[turn as usize % IN_FLIGHT].take();
                        drop(st);
                        let wait_ms = ms(tw);
                        let a = self.settings.attempts(turn);
                        let launched = self.submit_prepared(&streams[turn as usize % IN_FLIGHT], prepared, &a, slot);
                        let mut st = state.lock().expect("no panics");
                        let after = prev_state(&st.in_flight);
                        st.launch += 1;
                        match launched {
                            Ok(mut pending) => {
                                if let Some(p) = pending.prof.as_mut() {
                                    (p.take, p.fill, p.wait, p.before, p.after) = (take_ms, fill_ms, wait_ms, before, after);
                                }
                                st.in_flight.push_back((items, pending))
                            }
                            Err(e) => {
                                st.running -= 1;
                                st.failed.get_or_insert(e);
                                st.lost.extend(items);
                            }
                        }
                        changed.notify_all();
                    }
                    state.lock().expect("no panics").preparers -= 1;
                    changed.notify_all();
                });
            }
            loop {
                let next = {
                    let mut st = state.lock().expect("no panics");
                    loop {
                        if let Some(launch) = st.in_flight.pop_front() {
                            break Some(launch);
                        }
                        if st.preparers == 0 {
                            break None;
                        }
                        st = changed.wait(st).expect("no panics");
                    }
                };
                let Some((items, mut pending)) = next else { break };
                let ts = Instant::now();
                let waited = pending.stream.synchronize().map_err(err);
                let sync_ms = ms(ts);
                let prof = pending.prof.take();
                let tc = Instant::now();
                let collected = waited.and_then(|()| pending.collect_slot());
                let collect_ms = ms(tc);
                let collected = {
                    // The slot goes back for the next launch on this stream, which may go once this one is copied out.
                    let mut st = state.lock().expect("no panics");
                    st.running -= 1;
                    collected.map(|(result, slot)| {
                        st.slots[collected_turns % IN_FLIGHT] = Some(slot);
                        result
                    })
                };
                collected_turns += 1;
                changed.notify_all();
                match collected {
                    Ok(result) => {
                        let td = Instant::now();
                        done(items, result);
                        if let (Some(p), Some(b), Some(path)) = (prof, base.as_ref(), feedprof()) {
                            let at = |e: &CudaEvent| f64::from(b.elapsed_ms(e).unwrap_or(f32::NAN));
                            let (h, s0, m, e) = (at(&p.ev[0]), at(&p.ev[1]), at(&p.ev[2]), at(&p.ev[3]));
                            let q = &mut psum;
                            for (i, x) in [p.take, p.fill, p.wait, p.htod, p.alloc, p.launch, sync_ms, collect_ms, ms(td), s0 - h, e - m]
                                .into_iter()
                                .enumerate()
                            {
                                q.add(i, x);
                            }
                            if let Some(pm) = q.prev_mid {
                                let gap = s0 - pm;
                                q.add(11, gap.max(0.0));
                                q.add(12, m - pm);
                                q.add(13, m - s0.max(pm));
                                q.v[14] += gap;
                                q.gaps += u32::from(gap > 0.0);
                            }
                            q.prev_mid = Some(m);
                            q.before[usize::from(p.before.min(2))] += 1;
                            q.after[usize::from(p.after.min(2))] += 1;
                            q.n += 1;
                            if q.n == 50 {
                                let n = f64::from(q.n);
                                let names = ["take", "fill", "wait", "htod", "alloc", "launch", "sync", "collect", "done", "gpu_htod", "wl", "gap+", "cycle", "excl"];
                                let mut line = format!("{:.1}s n={n}", base_t.elapsed().as_secs_f64());
                                for (i, name) in names.iter().enumerate() {
                                    line += &format!(" {name}={:.2}/{:.2}", q.v[i] / n, q.mx[i]);
                                }
                                line += &format!(" gapmean={:.2} gaps>0={} before={:?} after={:?}\n", q.v[14] / n, q.gaps, q.before, q.after);
                                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                                    use std::io::Write;
                                    let _ = f.write_all(line.as_bytes());
                                }
                                let pm = q.prev_mid;
                                *q = ProfSum { prev_mid: pm, ..Default::default() };
                            }
                        }
                    }
                    Err(e) => {
                        let mut st = state.lock().expect("no panics");
                        st.failed.get_or_insert(e);
                        st.lost.extend(items);
                        while let Some((items, _)) = st.in_flight.pop_front() {
                            st.lost.extend(items);
                        }
                        changed.notify_all();
                    }
                }
            }
        });
        let st = state.into_inner().expect("no panics");
        match st.failed {
            Some(e) => Err((e, st.lost)),
            None => Ok(()),
        }
    }

    /// Random playouts from `positions`, recording the kernels' rules code at every step (for the tests).
    pub fn playouts(&self, positions: &[[u8; 52]], seed: u64) -> Result<Vec<Playout>, String> {
        let n = positions.len();
        let s = self.ctx.default_stream();
        let flat: Vec<u8> = positions.iter().flatten().copied().collect();
        let d_positions = s.clone_htod(&flat).map_err(err)?;
        let d_masks = s.clone_htod(&Prepared::new(positions, &|p: &[u8; 52]| *p).masks).map_err(err)?;
        let mut masks = s.alloc_zeros::<u64>(n * 52 * 2).map_err(err)?;
        let mut moves = s.alloc_zeros::<u8>(n * 52).map_err(err)?;
        let mut flags = s.alloc_zeros::<u8>(n * 52).map_err(err)?;
        let count = n as u32;
        let mut launch = s.launch_builder(&self.playouts);
        launch
            .arg(&d_positions)
            .arg(&d_masks)
            .arg(&count)
            .arg(&seed)
            .arg(&mut masks)
            .arg(&mut moves)
            .arg(&mut flags);
        let block = self.settings.block;
        let cfg = LaunchConfig {
            grid_dim: (count.div_ceil(block), 1, 1),
            block_dim: (block, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the argument list matches `playouts` in kernel.cu, and the outputs hold 52 steps per playout.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        let (masks, moves, flags) = (
            s.clone_dtoh(&masks).map_err(err)?,
            s.clone_dtoh(&moves).map_err(err)?,
            s.clone_dtoh(&flags).map_err(err)?,
        );
        Ok((0..n)
            .map(|p| {
                let steps = moves[p * 52..p * 52 + 52].iter().position(|&m| m == 0xFF).map_or(52, |i| i + 1);
                let at = p * 52..p * 52 + steps;
                Playout {
                    masks: at.clone().map(|i| (masks[2 * i], masks[2 * i + 1])).collect(),
                    moves: moves[at.clone()].to_vec(),
                    flags: flags[at].to_vec(),
                }
            })
            .collect())
    }
}

impl Pending {
    /// Waits for the batch and copies its results back.
    pub fn collect(self) -> Result<BatchResult, String> {
        self.collect_slot().map(|(result, _)| result)
    }

    /// [`Pending::collect`], giving back the slot for the next launch on this stream.
    fn collect_slot(self) -> Result<(BatchResult, Box<Slot>), String> {
        let s = &self.stream;
        let mut slot = self.slot;
        // Copies run in the order they are queued, whatever their stream: a download queued with its launch held
        // up the uploads of the next launches until this one's kernels had finished (FEED2: 2.9M deals/s against 5.3M).
        let Slot { won, lines, counts, down_won, down_lines, down_counts, .. } = &mut *slot;
        s.memcpy_dtoh(&*won, down_won).map_err(err)?;
        s.memcpy_dtoh(&*lines, down_lines).map_err(err)?;
        s.memcpy_dtoh(&*counts, down_counts).map_err(err)?;
        let profile = if self.profile { s.clone_dtoh(&slot.profile).map_err(err)? } else { Vec::new() };
        s.synchronize().map_err(err)?;
        let (mut won, mut lines, mut counts) = self.spare;
        won.clear();
        won.extend_from_slice(slot.down_won.as_slice().map_err(err)?);
        lines.clear();
        lines.extend_from_slice(slot.down_lines.as_slice().map_err(err)?);
        counts.clear();
        counts.extend_from_slice(slot.down_counts.as_slice().map_err(err)?);
        Ok((BatchResult {
            won,
            lines,
            counts,
            piles: self.piles,
            codes: self.codes,
            profile,
            masks: self.host_masks,
            pool: self.pool,
        }, slot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_past_the_counter_are_refused() {
        // The rungs' tries together must fit the kernel's 16-bit count of attempts per deal.
        let with = |rungs: Vec<[u32; 2]>| Settings { rungs, ..Settings::default() }.check();
        assert!(Settings::default().check().is_ok());
        assert!(with(vec![[38, MAX_TRIES]]).is_ok());
        assert!(with(vec![[38, 60_000], [36, 5_535]]).is_ok());
        assert!(with(vec![[38, 60_000], [36, 5_536]]).is_err());
        assert!(with(vec![[38, u32::MAX], [36, u32::MAX]]).is_err());
    }
}
