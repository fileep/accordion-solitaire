//! GPU ladder attempts (PLAN.md Phase 5 track F, Phase 8). The kernel (`kernel.cu`, compiled at run time with
//! NVRTC) runs one attempt per thread: a random prefix, then a small first-win search of what is left. The host
//! sends positions in batches short enough for Windows' GPU watchdog, and every win comes back as a move line for
//! the CPU to verify. A batch is submitted on a stream and collected later; [`Gpu::feed`] keeps two in flight, so
//! the GPU never waits for the host.
//!
//! `acc batch --gpu` runs this next to the CPU solver (crates/cli/src/runner.rs); `accgpu` measures it alone.

use accordion_core::Deal;
use accordion_core::fast::Position;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DriverError, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

const SOURCE: &str = include_str!("kernel.cu");

/// Fills a position's bytes after its last pile.
pub const SENTINEL: u8 = 0x7F;
/// The kernel's limits: random moves before a search, and piles a search can hold.
pub const MAX_PREFIX: u32 = 48;
pub const RESIDUAL: usize = 16;
/// Most attempts per position over both rungs: the kernel counts them in the low 16 bits of the position's counter.
pub const MAX_TRIES: u32 = 0xFFFF;
/// NVRTC 12.9 writes PTX that needs a driver for CUDA 12.9 or later.
const MIN_DRIVER: i32 = 12090;
/// The kernel's profile counters (kernel.cu, with [`Settings::profile`]), in order: cycles taking work, loading the
/// position, playing the prefix, the dead-end gate, searching, writing the result and skipping a won deal's attempt
/// (summed over threads); steps of the search loop by warps and by threads, and of the prefix loop; attempts taken
/// and skipped; searches by positions generated (0-15, 16-63, 64-255, 256-1023, 1024 up to the budget, the budget);
/// prefixes that got stuck, left more than [`RESIDUAL`] piles, or left a disconnected residual, and the moves the
/// stuck ones played (`attempts_stepwise` only); thread spans summed (ns); the latest end (ns); the earliest start,
/// bit-inverted (ns); and turns of `attempts_split`'s prefix threads with a residual its queue had no room for, and
/// of its search threads with none to take. In `attempts_split` the first four count its prefix warps (the fourth
/// queueing residuals), the next three its search warps (searching, taking residuals and writing results, waiting
/// for residuals); it leaves the prefix and search tallies to `attempts_stepwise`, whose attempts are the same.
pub const PROFILE_SLOTS: usize = 28;

/// How `acc batch` uses the GPU (`--gpu-config`, JSON; fields left out keep these defaults, measured in Phase 5
/// track F).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// One or two rungs, as [random moves before the search, tries per deal]. Both run in one launch, the second
    /// on the deals the first didn't win. Deals need at least 36 random moves to leave the search ≤16 piles.
    pub rungs: Vec<[u32; 2]>,
    /// Positions each search may generate.
    pub budget: u32,
    /// Deals per launch: keep launches well under Windows' 2 s GPU watchdog (65,536 take about 0.1 s on the T550, and
    /// fewer leave several attempts at once on each deal, which wastes those still running once it is won).
    pub batch: usize,
    /// Threads per block (the kernel's shared memory is sized for it: 64 bytes per thread). On the T550, 64 fits
    /// 1,024 attempts per multiprocessor, its maximum (32 fits 512, the most blocks it takes); with `split`, whose
    /// queue takes shared memory too, 8 blocks of 64 (one prefix warp, one search warp) fit, the default: 8 of 64 blocks
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
    /// Run attempts a step at a time (kernel.cu `attempts_stepwise`), so a warp's threads step together: 1.7 times
    /// as many deals a second on the T550 as `attempts`, where each thread runs its own attempts start to finish.
    pub stepwise: bool,
    /// Play prefixes and search in different warps (kernel.cu `attempts_split`; `stepwise` then doesn't matter):
    /// `prefix_warps` of a block's warps play prefixes and queue the residuals for the others to search, so neither
    /// waits for the other's step; the prefix's row is in registers and its warps go in lockstep. About 3.5 times the
    /// deals a second of `attempts_stepwise` on the T550. `block` must be a multiple of 32.
    pub split: bool,
    /// The warps of a block that play prefixes with `split` (kernel.cu `PREFIX_THREADS`); `block` must hold more.
    pub prefix_warps: u32,
    /// Shared early prefix moves (`split` only, the other kernels ignore them; kernel.cu `SHARE_G` and `SHARE_K`): a
    /// deal's tries go in groups of `share_group`, whose first `share_moves` prefix moves are played once; each try
    /// then plays the rest of its prefix from the row they left, and searches. 1 shares nothing.
    pub share_group: u32,
    /// From 20 (the masks a group keeps then fit 32 bits) up to the shorter rung's random moves when `share_group` is
    /// more than 1.
    pub share_moves: u32,
    /// The most ranks a residual of each rung may have to be searched (`split` only; kernel.cu `RANK_GATE`).
    pub rank_gate: [u32; 2],
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            rungs: vec![[39, 128], [37, 1152]],
            budget: 700,
            batch: 65536,
            block: 64,
            blocks_per_sm: 8,
            conn_gate: 19,
            leftmost: true,
            leap_weight: 8,
            seed: 0x5EED,
            profile: false,
            stepwise: true,
            split: true,
            prefix_warps: 1,
            share_group: 4,
            share_moves: 20,
            rank_gate: [9, 10],
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
        if self.split
            && (!self.block.is_multiple_of(32)
                || self.prefix_warps == 0
                || self.prefix_warps >= self.block / 32)
        {
            return Err(format!(
                "block {}, prefix_warps {}: with split, whole warps, at least one playing prefixes and one searching",
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

    /// The attempt kernel these settings run.
    pub fn kernel(&self) -> &'static str {
        match (self.split, self.stepwise) {
            (true, _) => "attempts_split",
            (false, true) => "attempts_stepwise",
            (false, false) => "attempts",
        }
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
    won: CudaSlice<u32>,
    lines: CudaSlice<u8>,
    /// Per position: positions generated << 16 | attempts run.
    counts: CudaSlice<u64>,
    profile: Option<CudaSlice<u64>>,
    // Kept alive until the kernel is done with them.
    _inputs: (CudaSlice<u8>, CudaSlice<u64>, CudaSlice<u32>, CudaSlice<u32>),
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
    /// [`Settings::kernel`].
    attempts: CudaFunction,
    write_lines: CudaFunction,
    playouts: CudaFunction,
    pub name: String,
    pub sms: u32,
    pub settings: Settings,
}

fn err(e: DriverError) -> String {
    format!("{e:?}")
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
const IN_FLIGHT: usize = 2;

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
                        let (l1, l3, present) = Position::masks_of_codes(&codes[..n]);
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
            let (l1, l3, present) = Position::masks_of_codes(&codes[..n]);
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

/// Finds the NVIDIA driver and NVRTC without letting cudarc panic when one is missing, adding the copy of NVRTC
/// in `.tools/` (PLAN.md Phase 0 notes) to PATH when NVRTC isn't on it.
fn find_libraries() -> Result<(), String> {
    // SAFETY: loads and unloads the NVIDIA driver library, which cudarc would load anyway.
    if !unsafe { cudarc::driver::sys::is_culib_present() } {
        return Err("no NVIDIA driver found".into());
    }
    let mut version = 0;
    // SAFETY: cuDriverGetVersion only writes the version; it needs no initialization.
    unsafe { cudarc::driver::sys::cuDriverGetVersion(&mut version) }.result().map_err(err)?;
    if version < MIN_DRIVER {
        return Err(format!(
            "the NVIDIA driver supports CUDA {}.{}; the kernel needs 12.9 or later",
            version / 1000,
            version % 1000 / 10
        ));
    }
    // SAFETY: as above, for NVRTC.
    let nvrtc = || unsafe { cudarc::nvrtc::sys::is_culib_present() };
    if !nvrtc() {
        add_bundled_nvrtc();
        if !nvrtc() {
            return Err(
                "NVRTC, the CUDA runtime compiler, not found: put its bin folder on PATH (PLAN.md Phase 0 notes)"
                    .into(),
            );
        }
    }
    Ok(())
}

/// `.tools/cuda-nvrtc-*/nvidia/cuda_nvrtc/bin` in the current directory or the executable's, or above them.
#[cfg(windows)]
fn add_bundled_nvrtc() {
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    let found = starts.iter().flatten().flat_map(|p| p.ancestors()).find_map(|dir| {
        let mut copies: Vec<_> = std::fs::read_dir(dir.join(".tools"))
            .ok()?
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("cuda-nvrtc"))
            .map(|e| e.path().join("nvidia/cuda_nvrtc/bin"))
            .filter(|bin| bin.is_dir())
            .collect();
        copies.sort();
        copies.pop()
    });
    if let Some(bin) = found {
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = std::env::join_paths(paths) {
            // SAFETY: changing the environment is always safe on Windows (see std::env::set_var).
            unsafe { std::env::set_var("PATH", path) };
        }
    }
}

#[cfg(not(windows))]
fn add_bundled_nvrtc() {}

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
    .map_err(|e| format!("compiling the kernel: {e:?}"))
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
        let ctx = CudaContext::new(0).map_err(|e| format!("no CUDA device: {e:?}"))?;
        // Wait for the GPU by sleeping, not spinning, so the CPU core stays free for the CPU solver.
        ctx.set_blocking_synchronize().map_err(err)?;
        let name = ctx.name().unwrap_or_default();
        let sms = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT).map_err(err)? as u32;
        check_defines(source, defines)?;
        let ptx = compile(&ctx, &settings, source, defines)?;
        let module = ctx.load_module(ptx).map_err(|e| format!("loading the kernel: {e:?}"))?;
        let attempts = module.load_function(settings.kernel()).map_err(err)?;
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
        let ctx = CudaContext::new(0).map_err(|e| format!("no CUDA device: {e:?}"))?;
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
        self.submit_prepared(stream, Prepared::new(positions, &|p: &[u8; 52]| *p), a)
    }

    fn submit_prepared(&self, stream: &Arc<CudaStream>, prepared: Prepared, a: &Attempts) -> Result<Pending, String> {
        if a.tries.saturating_add(a.tries2) > MAX_TRIES {
            return Err(format!(
                "{} tries per position in all; at most {MAX_TRIES}",
                a.tries.saturating_add(a.tries2)
            ));
        }
        let count = prepared.piles.len();
        let n = count as u32;
        let s = stream;
        let d_positions = s.clone_htod(&prepared.flat).map_err(err)?;
        let d_masks = s.clone_htod(&prepared.masks).map_err(err)?;
        let inv = reciprocal(count);
        let mut next = s.alloc_zeros::<u32>(1).map_err(err)?;
        let mut won = s.alloc_zeros::<u32>(count).map_err(err)?;
        let mut lines = s.alloc_zeros::<u8>(count * 51).map_err(err)?;
        let mut counts = s.alloc_zeros::<u64>(count).map_err(err)?;
        let mut profile = s.alloc_zeros::<u64>(PROFILE_SLOTS).map_err(err)?;
        let leftmost = u32::from(a.leftmost);
        let mut launch = s.launch_builder(&self.attempts);
        launch
            .arg(&d_positions)
            .arg(&d_masks)
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
            .arg(&mut next)
            .arg(&mut won)
            .arg(&mut lines)
            .arg(&mut counts)
            .arg(&mut profile);
        let cfg = LaunchConfig {
            grid_dim: (self.blocks(), 1, 1),
            block_dim: (self.settings.block, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the argument list matches the kernel's signature in kernel.cu, and every buffer is sized for
        // `n` positions as the kernel indexes them.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        let mut next_line = s.alloc_zeros::<u32>(1).map_err(err)?;
        if self.settings.split {
            // On the same stream, so after the attempts are done.
            let mut launch = s.launch_builder(&self.write_lines);
            launch
                .arg(&d_positions)
                .arg(&d_masks)
                .arg(&n)
                .arg(&a.tries)
                .arg(&a.prefix)
                .arg(&a.tries2)
                .arg(&a.prefix2)
                .arg(&a.leap_shift)
                .arg(&a.seed)
                .arg(&won)
                .arg(&mut lines)
                .arg(&mut next_line);
            let cfg = LaunchConfig { grid_dim: (self.blocks(), 1, 1), block_dim: (32, 1, 1), shared_mem_bytes: 0 };
            // SAFETY: the argument list matches write_lines's signature in kernel.cu, and every buffer is sized for `n`
            // positions as the kernel indexes them.
            unsafe { launch.launch(cfg) }.map_err(err)?;
        }
        Ok(Pending {
            stream: s.clone(),
            piles: prepared.piles,
            codes: prepared.flat,
            host_masks: prepared.masks,
            spare: prepared.spare,
            pool: prepared.pool,
            won,
            lines,
            counts,
            profile: self.settings.profile.then_some(profile),
            _inputs: (d_positions, d_masks, next, next_line),
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
        });
        let changed = Condvar::new();
        let pool: Pool = Arc::default();
        let (position, streams) = (&position, &streams);
        std::thread::scope(|scope| {
            for _ in 0..PREPARERS {
                scope.spawn(|| {
                    loop {
                        let taken = {
                            let mut t = taking.lock().expect("no panics");
                            if state.lock().expect("no panics").failed.is_some() {
                                None
                            } else {
                                (t.take)().map(|items| {
                                    t.turn += 1;
                                    (t.turn - 1, items)
                                })
                            }
                        };
                        let Some((turn, items)) = taken else { break };
                        let prepared = Prepared::fill(&pool, &items, position);
                        let mut st = state.lock().expect("no panics");
                        while (st.launch != turn || st.running >= IN_FLIGHT) && st.failed.is_none() {
                            st = changed.wait(st).expect("no panics");
                        }
                        if st.failed.is_some() {
                            st.lost.extend(items);
                            break;
                        }
                        st.running += 1;
                        drop(st);
                        let a = self.settings.attempts(turn);
                        let launched = self.submit_prepared(&streams[turn as usize % IN_FLIGHT], prepared, &a);
                        let mut st = state.lock().expect("no panics");
                        st.launch += 1;
                        match launched {
                            Ok(pending) => st.in_flight.push_back((items, pending)),
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
                let Some((items, pending)) = next else { break };
                let waited = pending.stream.synchronize().map_err(err);
                state.lock().expect("no panics").running -= 1;
                changed.notify_all();
                match waited.and_then(|()| pending.collect()) {
                    Ok(result) => done(items, result),
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
        let s = &self.stream;
        let (mut won, mut lines, mut counts) = self.spare;
        won.resize(self.won.len(), 0);
        lines.resize(self.lines.len(), 0);
        counts.resize(self.counts.len(), 0);
        s.memcpy_dtoh(&self.won, &mut won).map_err(err)?;
        s.memcpy_dtoh(&self.lines, &mut lines).map_err(err)?;
        s.memcpy_dtoh(&self.counts, &mut counts).map_err(err)?;
        let profile = match &self.profile {
            Some(p) => s.clone_dtoh(p).map_err(err)?,
            None => Vec::new(),
        };
        s.synchronize().map_err(err)?;
        Ok(BatchResult {
            won,
            lines,
            counts,
            piles: self.piles,
            codes: self.codes,
            profile,
            masks: self.host_masks,
            pool: self.pool,
        })
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
