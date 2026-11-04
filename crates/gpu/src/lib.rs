//! GPU ladder attempts (PLAN.md Phase 5 track F). The kernel (`kernel.cu`, compiled at run time with NVRTC) runs
//! one attempt per thread: a random prefix, then a small first-win search of what is left. The host sends deals in
//! batches short enough for Windows' GPU watchdog, and every win comes back as a move line for the CPU to verify.
//! A batch is submitted on a stream and collected later, so a caller can keep two in flight and the GPU never
//! waits for the host.

use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DriverError, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};
use std::sync::Arc;

const SOURCE: &str = include_str!("kernel.cu");

/// Settings for one batch of attempts: two rungs in one launch.
#[derive(Clone, Copy, Debug)]
pub struct Attempts {
    /// Attempts per deal on the first rung, and random moves before each one's search.
    pub tries: u32,
    pub prefix: u32,
    /// The same for the second rung (0 tries: none).
    pub tries2: u32,
    pub prefix2: u32,
    /// Positions each search may generate.
    pub budget: u32,
    /// Connectivity test at positions with at most this many piles.
    pub conn_gate: u32,
    pub seed: u64,
}

/// What came back for a batch.
pub struct BatchResult {
    /// Per deal: the winning line (51 compact moves, `pile << 1 | leap`), if an attempt won it.
    pub lines: Vec<Option<Vec<u8>>>,
    pub nodes: u64,
    pub attempts_run: u32,
}

/// A launched batch, until its results are copied back.
pub struct Pending {
    stream: Arc<CudaStream>,
    won: CudaSlice<u32>,
    lines: CudaSlice<u8>,
    nodes: CudaSlice<u64>,
    runs: CudaSlice<u32>,
    // Kept alive until the kernel is done with them.
    _inputs: (CudaSlice<u8>, CudaSlice<u32>, CudaSlice<u32>),
}

pub struct Gpu {
    ctx: Arc<CudaContext>,
    kernel: CudaFunction,
    pub name: String,
    pub sms: u32,
    /// Threads per block the kernel was compiled for (its shared-memory arrays are sized by it).
    pub block: u32,
}

fn err(e: DriverError) -> String {
    format!("{e:?}")
}

impl Gpu {
    /// Compiles the kernel for device 0, for blocks of `block` threads.
    pub fn new(block: u32) -> Result<Gpu, String> {
        use cudarc::driver::sys::CUdevice_attribute as A;
        let ctx = CudaContext::new(0).map_err(|e| format!("CUDA device: {e:?}"))?;
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
                options: vec![format!("-DBLOCK={block}")],
                ..Default::default()
            },
        )
        .map_err(|e| format!("compiling the kernel: {e:?}"))?;
        let module = ctx.load_module(ptx).map_err(|e| format!("loading the kernel: {e:?}"))?;
        let kernel = module.load_function("attempts").map_err(err)?;
        Ok(Gpu { ctx, kernel, name, sms, block })
    }

    /// A new stream to submit batches on.
    pub fn stream(&self) -> Result<Arc<CudaStream>, String> {
        self.ctx.new_stream().map_err(err)
    }

    /// Launches attempts on each deal (52 engine bytes each) and returns at once; `blocks` blocks of persistent
    /// threads share the work, and a deal's remaining attempts are skipped once one has won it.
    pub fn submit(
        &self,
        stream: &Arc<CudaStream>,
        deals: &[[u8; 52]],
        a: &Attempts,
        blocks: u32,
    ) -> Result<Pending, String> {
        let n = deals.len() as u32;
        let flat: Vec<u8> = deals.iter().flatten().copied().collect();
        let s = stream;
        let d_deals = s.clone_htod(&flat).map_err(err)?;
        let mut next = s.alloc_zeros::<u32>(1).map_err(err)?;
        let mut won = s.alloc_zeros::<u32>(deals.len()).map_err(err)?;
        let mut lines = s.alloc_zeros::<u8>(deals.len() * 51).map_err(err)?;
        let mut nodes = s.alloc_zeros::<u64>(1).map_err(err)?;
        let mut runs = s.alloc_zeros::<u32>(1).map_err(err)?;
        let mut wins = s.alloc_zeros::<u32>(1).map_err(err)?;
        let mut launch = s.launch_builder(&self.kernel);
        launch
            .arg(&d_deals)
            .arg(&n)
            .arg(&a.tries)
            .arg(&a.prefix)
            .arg(&a.tries2)
            .arg(&a.prefix2)
            .arg(&a.budget)
            .arg(&a.conn_gate)
            .arg(&a.seed)
            .arg(&mut next)
            .arg(&mut won)
            .arg(&mut lines)
            .arg(&mut nodes)
            .arg(&mut runs)
            .arg(&mut wins);
        let cfg =
            LaunchConfig { grid_dim: (blocks, 1, 1), block_dim: (self.block, 1, 1), shared_mem_bytes: 0 };
        // SAFETY: the argument list matches the kernel's signature in kernel.cu, and every buffer is sized for
        // `n` deals as the kernel indexes them.
        unsafe { launch.launch(cfg) }.map_err(err)?;
        Ok(Pending { stream: s.clone(), won, lines, nodes, runs, _inputs: (d_deals, next, wins) })
    }

    /// [`Gpu::submit`] and wait for the result.
    pub fn run(&self, deals: &[[u8; 52]], a: &Attempts, blocks: u32) -> Result<BatchResult, String> {
        let stream = self.ctx.default_stream();
        self.submit(&stream, deals, a, blocks)?.collect()
    }
}

impl Pending {
    /// Waits for the batch and copies its results back.
    pub fn collect(self) -> Result<BatchResult, String> {
        let s = &self.stream;
        let won = s.clone_dtoh(&self.won).map_err(err)?;
        let lines = s.clone_dtoh(&self.lines).map_err(err)?;
        let nodes = s.clone_dtoh(&self.nodes).map_err(err)?[0];
        let runs = s.clone_dtoh(&self.runs).map_err(err)?[0];
        Ok(BatchResult {
            lines: won
                .iter()
                .enumerate()
                .map(|(i, &w)| (w != 0).then(|| lines[i * 51..i * 51 + 51].to_vec()))
                .collect(),
            nodes,
            attempts_run: runs,
        })
    }
}
