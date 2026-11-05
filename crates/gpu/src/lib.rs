//! GPU ladder attempts (PLAN.md Phase 5 track F, Phase 8). The kernel (`kernel.cu`, compiled at run time with
//! NVRTC) runs one attempt per thread: a random prefix, then a small first-win search of what is left. The host
//! sends positions in batches short enough for Windows' GPU watchdog, and every win comes back as a move line for
//! the CPU to verify. A batch is submitted on a stream and collected later; [`Gpu::feed`] keeps two in flight, so
//! the GPU never waits for the host.
//!
//! `acc batch --gpu` runs this next to the CPU solver (crates/cli/src/runner.rs); `accgpu` measures it alone.

use accordion_core::Deal;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DriverError, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

const SOURCE: &str = include_str!("kernel.cu");

/// Fills a position's bytes after its last pile.
pub const SENTINEL: u8 = 0x7F;
/// The kernel's limits: random moves before a search, and piles a search can hold.
pub const MAX_PREFIX: u32 = 48;
pub const RESIDUAL: usize = 16;
/// NVRTC 12.9 writes PTX that needs a driver for CUDA 12.9 or later.
const MIN_DRIVER: i32 = 12090;

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
    /// Deals per launch: keep launches well under Windows' 2 s GPU watchdog.
    pub batch: usize,
    /// Threads per block (the kernel's shared memory is sized for it: 96 bytes per thread). Shared memory sets how
    /// many attempts a multiprocessor holds; on the T550, 64 fits 640 (32 fits 512, the most blocks it takes).
    pub block: u32,
    /// Thread blocks per multiprocessor.
    pub blocks_per_sm: u32,
    /// Connectivity test at positions with at most this many piles.
    pub conn_gate: u32,
    /// The stronger connectivity test that leaves pile 0's card out (accordion_core::fast's
    /// `connected_without_leftmost`): about 50% faster on the T550.
    pub leftmost: bool,
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            rungs: vec![[38, 256], [36, 1024]],
            budget: 2000,
            batch: 4096,
            block: 64,
            blocks_per_sm: 16,
            conn_gate: 19,
            leftmost: true,
            seed: 0x5EED,
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
        if self.batch == 0 || self.block == 0 || self.block > 256 || self.blocks_per_sm == 0 {
            return Err("batch, block (at most 256) and blocks_per_sm must be positive".into());
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
    pub seed: u64,
}

/// What came back for a batch, per position.
pub struct BatchResult {
    /// The winning line (compact moves, `pile << 1 | leap`), if an attempt won it.
    pub lines: Vec<Option<Vec<u8>>>,
    /// Positions generated, prefix moves included, over every attempt.
    pub nodes: Vec<u64>,
    /// Attempts run (a position's later attempts are skipped once one has won it).
    pub tries: Vec<u32>,
}

/// A launched batch, until its results are copied back.
pub struct Pending {
    stream: Arc<CudaStream>,
    /// Piles of each position: its line has one move fewer.
    piles: Vec<usize>,
    won: CudaSlice<u32>,
    lines: CudaSlice<u8>,
    nodes: CudaSlice<u64>,
    tries: CudaSlice<u32>,
    // Kept alive until the kernel is done with them.
    _inputs: (CudaSlice<u8>, CudaSlice<u32>),
}

/// What [`Gpu::playouts`] recorded for one playout, step by step (kernel.cu `playouts`).
pub struct Playout {
    /// The position's slide and leap masks (bit i: pile i may move).
    pub masks: Vec<(u64, u64)>,
    /// The move played from it, `pile << 1 | leap`; the last step has none.
    pub moves: Vec<u8>,
    /// Bit 0: connected; bit 1: the move before broke a link; bit 2: undo failed.
    pub flags: Vec<u8>,
}

pub struct Gpu {
    ctx: Arc<CudaContext>,
    attempts: CudaFunction,
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

impl Gpu {
    /// Compiles the kernels for device 0 with `settings` (its block size), or says why there is no usable GPU.
    pub fn new(settings: Settings) -> Result<Gpu, String> {
        use cudarc::driver::sys::CUdevice_attribute as A;
        settings.check()?;
        find_libraries()?;
        let ctx = CudaContext::new(0).map_err(|e| format!("no CUDA device: {e:?}"))?;
        // Wait for the GPU by sleeping, not spinning, so the CPU core stays free for the CPU solver.
        ctx.set_blocking_synchronize().map_err(err)?;
        let name = ctx.name().unwrap_or_default();
        let sms = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_MULTIPROCESSOR_COUNT).map_err(err)? as u32;
        let major = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR).map_err(err)?;
        let minor = ctx.attribute(A::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR).map_err(err)?;
        let arch: &'static str = Box::leak(format!("compute_{major}{minor}").into_boxed_str());
        let ptx = compile_ptx_with_opts(
            SOURCE,
            CompileOptions {
                arch: Some(arch),
                options: vec![format!("-DBLOCK={}", settings.block)],
                ..Default::default()
            },
        )
        .map_err(|e| format!("compiling the kernel: {e:?}"))?;
        let module = ctx.load_module(ptx).map_err(|e| format!("loading the kernel: {e:?}"))?;
        let attempts = module.load_function("attempts").map_err(err)?;
        let playouts = module.load_function("playouts").map_err(err)?;
        Ok(Gpu { ctx, attempts, playouts, name, sms, settings })
    }

    /// Thread blocks per launch.
    pub fn blocks(&self) -> u32 {
        self.sms * self.settings.blocks_per_sm
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
        let n = positions.len() as u32;
        let flat: Vec<u8> = positions.iter().flatten().copied().collect();
        let s = stream;
        let d_positions = s.clone_htod(&flat).map_err(err)?;
        let mut next = s.alloc_zeros::<u32>(1).map_err(err)?;
        let mut won = s.alloc_zeros::<u32>(positions.len()).map_err(err)?;
        let mut lines = s.alloc_zeros::<u8>(positions.len() * 51).map_err(err)?;
        let mut nodes = s.alloc_zeros::<u64>(positions.len()).map_err(err)?;
        let mut tries = s.alloc_zeros::<u32>(positions.len()).map_err(err)?;
        let leftmost = u32::from(a.leftmost);
        let mut launch = s.launch_builder(&self.attempts);
        launch
            .arg(&d_positions)
            .arg(&n)
            .arg(&a.tries)
            .arg(&a.prefix)
            .arg(&a.tries2)
            .arg(&a.prefix2)
            .arg(&a.budget)
            .arg(&a.conn_gate)
            .arg(&leftmost)
            .arg(&a.seed)
            .arg(&mut next)
            .arg(&mut won)
            .arg(&mut lines)
            .arg(&mut nodes)
            .arg(&mut tries);
        let cfg = LaunchConfig {
            grid_dim: (self.blocks(), 1, 1),
            block_dim: (self.settings.block, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the argument list matches the kernel's signature in kernel.cu, and every buffer is sized for
        // `n` positions as the kernel indexes them.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        Ok(Pending {
            stream: s.clone(),
            piles: positions.iter().map(piles).collect(),
            won,
            lines,
            nodes,
            tries,
            _inputs: (d_positions, next),
        })
    }

    /// [`Gpu::submit`] and wait for the result.
    pub fn run(&self, positions: &[[u8; 52]], a: &Attempts) -> Result<BatchResult, String> {
        let stream = self.ctx.default_stream();
        self.submit(&stream, positions, a)?.collect()
    }

    /// Runs the settings' rungs on batches of items from `next_batch` (`position` gives each one's bytes),
    /// keeping two launches in flight on two streams, and hands every finished batch to `done`. On a GPU error,
    /// returns it with the items whose results never came back.
    pub fn feed<T>(
        &self,
        position: impl Fn(&T) -> [u8; 52],
        mut next_batch: impl FnMut() -> Option<Vec<T>>,
        mut done: impl FnMut(Vec<T>, BatchResult),
    ) -> Result<(), (String, Vec<T>)> {
        let streams = match (self.stream(), self.stream()) {
            (Ok(a), Ok(b)) => [a, b],
            (Err(e), _) | (_, Err(e)) => return Err((e, Vec::new())),
        };
        let mut in_flight: VecDeque<(Vec<T>, Pending)> = VecDeque::new();
        let mut launch = 0u64;
        let lost = |in_flight: VecDeque<(Vec<T>, Pending)>, more: Vec<T>| {
            in_flight.into_iter().flat_map(|(items, _)| items).chain(more).collect::<Vec<T>>()
        };
        loop {
            if in_flight.len() < 2
                && let Some(items) = next_batch()
            {
                let positions: Vec<[u8; 52]> = items.iter().map(&position).collect();
                let a = self.settings.attempts(launch);
                match self.submit(&streams[(launch % 2) as usize], &positions, &a) {
                    Ok(pending) => in_flight.push_back((items, pending)),
                    Err(e) => return Err((e, lost(in_flight, items))),
                }
                launch += 1;
                continue;
            }
            let Some((items, pending)) = in_flight.pop_front() else { return Ok(()) };
            match pending.collect() {
                Ok(result) => done(items, result),
                Err(e) => return Err((e, lost(in_flight, items))),
            }
        }
    }

    /// Random playouts from `positions`, recording the kernels' rules code at every step (for the tests).
    pub fn playouts(&self, positions: &[[u8; 52]], seed: u64) -> Result<Vec<Playout>, String> {
        let n = positions.len();
        let s = self.ctx.default_stream();
        let flat: Vec<u8> = positions.iter().flatten().copied().collect();
        let d_positions = s.clone_htod(&flat).map_err(err)?;
        let mut masks = s.alloc_zeros::<u64>(n * 52 * 2).map_err(err)?;
        let mut moves = s.alloc_zeros::<u8>(n * 52).map_err(err)?;
        let mut flags = s.alloc_zeros::<u8>(n * 52).map_err(err)?;
        let count = n as u32;
        let mut launch = s.launch_builder(&self.playouts);
        launch.arg(&d_positions).arg(&count).arg(&seed).arg(&mut masks).arg(&mut moves).arg(&mut flags);
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
        let won = s.clone_dtoh(&self.won).map_err(err)?;
        let lines = s.clone_dtoh(&self.lines).map_err(err)?;
        let nodes = s.clone_dtoh(&self.nodes).map_err(err)?;
        let tries = s.clone_dtoh(&self.tries).map_err(err)?;
        Ok(BatchResult {
            lines: won
                .iter()
                .zip(&self.piles)
                .enumerate()
                .map(|(i, (&w, &piles))| (w != 0).then(|| lines[i * 51..i * 51 + piles - 1].to_vec()))
                .collect(),
            nodes,
            tries,
        })
    }
}
