//! Bounded persistent CUDA buffers for deterministic plane arrangements.
use super::normalize::{SolidVertex, VertexAccumulator};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};
use glam::DVec3;
use std::sync::{Arc, Mutex};

const TILE: usize = 262_144;
const MAX_PLANES: usize = 4096;
const BLOCK: usize = 256;

pub(super) struct Runtime {
    context: Arc<CudaContext>,
    pub clips: super::clipping::Clipping,
    prune: CudaFunction,
    pair_scan: CudaFunction,
    pair_offsets: CudaFunction,
    vertices: CudaFunction,
    prefix: CudaFunction,
    compact: CudaFunction,
    scratch: Mutex<Option<Scratch>>,
}

pub(super) struct Enumeration {
    pub verts: Vec<SolidVertex>,
    pub escaping: Option<Vec<usize>>,
    pub work: Work,
    pub triples: Vec<([usize; 3], DVec3)>,
}

#[derive(Default)]
pub(super) struct Work {
    pub tiles: u64,
    pub triples: u64,
    pub surviving: u64,
    pub candidates: u64,
    pub readback_bytes: u64,
    pub scratch_bytes: u64,
}

struct Scratch {
    stream: Arc<CudaStream>,
    capacity: usize,
    planes: CudaSlice<f64>,
    active: CudaSlice<u32>,
    pair_sums: CudaSlice<u32>,
    pair_offsets: CudaSlice<u64>,
    candidates: CudaSlice<f64>,
    ranks: CudaSlice<u32>,
    blocks: CudaSlice<u32>,
    output: CudaSlice<f64>,
    ordinals: CudaSlice<u64>,
    total: CudaSlice<u32>,
    escaped: CudaSlice<u32>,
    escape: CudaSlice<u32>,
}

impl Scratch {
    fn new(context: &Arc<CudaContext>, capacity: usize) -> Result<Self, String> {
        let stream = context.new_stream().map_err(|e| e.to_string())?;
        Ok(Self {
            capacity,
            planes: stream
                .alloc_zeros(capacity * 4)
                .map_err(|e| e.to_string())?,
            active: stream
                .alloc_zeros(capacity * capacity)
                .map_err(|e| e.to_string())?,
            pair_sums: stream
                .alloc_zeros((capacity * capacity).div_ceil(BLOCK))
                .map_err(|e| e.to_string())?,
            pair_offsets: stream
                .alloc_zeros((capacity * capacity).div_ceil(BLOCK) + 1)
                .map_err(|e| e.to_string())?,
            candidates: stream.alloc_zeros(TILE * 3).map_err(|e| e.to_string())?,
            ranks: stream.alloc_zeros(TILE).map_err(|e| e.to_string())?,
            blocks: stream
                .alloc_zeros(TILE / BLOCK)
                .map_err(|e| e.to_string())?,
            output: stream.alloc_zeros(TILE * 3).map_err(|e| e.to_string())?,
            ordinals: stream.alloc_zeros(TILE).map_err(|e| e.to_string())?,
            total: stream.alloc_zeros(2).map_err(|e| e.to_string())?,
            escaped: stream.alloc_zeros(1).map_err(|e| e.to_string())?,
            escape: stream.alloc_zeros(capacity).map_err(|e| e.to_string())?,
            stream,
        })
    }

    fn bytes(&self) -> u64 {
        (self.capacity * 32
            + self.capacity * self.capacity * 4
            + TILE * 60
            + TILE / BLOCK * 4
            + (self.capacity * self.capacity).div_ceil(BLOCK) * 12
            + 8
            + 12
            + self.capacity * 4) as u64
    }
}

impl Runtime {
    pub fn new(ordinal: usize) -> Result<Self, String> {
        let context = CudaContext::new(ordinal).map_err(|e| e.to_string())?;
        let (major, minor) = context.compute_capability().map_err(|e| e.to_string())?;
        let ptx = compile_ptx_with_opts(
            include_str!("vertices.cu"),
            CompileOptions {
                fmad: Some(false),
                use_fast_math: Some(false),
                prec_div: Some(true),
                prec_sqrt: Some(true),
                options: vec![format!("--gpu-architecture=compute_{major}{minor}")],
                ..CompileOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let module = context.load_module(ptx).map_err(|e| e.to_string())?;
        Ok(Self {
            clips: super::clipping::Clipping::new(Arc::clone(&context))?,
            prune: module
                .load_function("prune_pairs")
                .map_err(|e| e.to_string())?,
            pair_scan: module
                .load_function("pair_scan")
                .map_err(|e| e.to_string())?,
            pair_offsets: module
                .load_function("pair_offsets")
                .map_err(|e| e.to_string())?,
            vertices: module
                .load_function("vertices")
                .map_err(|e| e.to_string())?,
            prefix: module
                .load_function("prefix_blocks")
                .map_err(|e| e.to_string())?,
            compact: module.load_function("compact").map_err(|e| e.to_string())?,
            context,
            scratch: Mutex::new(None),
        })
    }

    pub fn tool_faces(
        &self,
        planes: &[(DVec3, f64)],
        centre: DVec3,
        radius: f64,
    ) -> Result<Vec<Vec<DVec3>>, String> {
        self.clips.faces(planes, centre, radius)
    }

    pub fn enumerate(&self, planes: &[(DVec3, f64)]) -> Result<Enumeration, String> {
        self.enumerate_impl(planes, false)
    }

    pub fn triples(&self, planes: &[(DVec3, f64)]) -> Result<Enumeration, String> {
        self.enumerate_impl(planes, true)
    }

    fn enumerate_impl(
        &self,
        planes: &[(DVec3, f64)],
        collect_triples: bool,
    ) -> Result<Enumeration, String> {
        let count = planes
            .len()
            .checked_add(6)
            .ok_or("solid plane count overflow")?;
        if count > MAX_PLANES {
            return Err(format!(
                "CUDA solid supports at most {} real planes",
                MAX_PLANES - 6
            ));
        }
        let mut input = Vec::with_capacity(count * 4);
        for &(n, m) in planes {
            input.extend([n.x, n.y, n.z, m]);
        }
        for n in [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ] {
            input.extend([n.x, n.y, n.z, 64.0]);
        }
        // Submission, readback and incremental vertex dedup share this scratch
        // lock. Face rings, calipers and mesh construction run after its release.
        let mut guard = self.scratch.lock().map_err(|e| e.to_string())?;
        if guard.as_ref().is_none_or(|s| s.capacity < count) {
            *guard = None;
            *guard = Some(Scratch::new(&self.context, count.next_power_of_two())?);
        }
        let s = guard.as_mut().ok_or("missing CUDA solid scratch")?;
        let (surviving, pair_blocks) = self.prepare_pairs(s, &input, count)?;
        let triples = count as u64 * (count as u64 - 1) * (count as u64 - 2) / 6;
        let mut work = Work {
            triples,
            surviving,
            readback_bytes: 8,
            scratch_bytes: s.bytes(),
            ..Work::default()
        };
        let mut acc = VertexAccumulator::default();
        let mut captured = Vec::new();
        let mut start = 0;
        while start < surviving {
            let n = (surviving - start).min(TILE as u64) as u32;
            let (total, escaped) =
                self.dispatch_tile(s, count, pair_blocks, start, n, collect_triples)?;
            work.tiles += 1;
            work.candidates += total as u64;
            work.readback_bytes += 8;
            if total > 0 && !escaped {
                read_candidates(
                    s,
                    total,
                    count,
                    collect_triples,
                    &mut acc,
                    &mut captured,
                    &mut work,
                )?;
            }
            start += u64::from(n);
        }
        let escaping = read_escaping(s, planes.len(), &mut work)?;
        Ok(Enumeration {
            verts: acc.verts,
            escaping,
            work,
            triples: captured,
        })
    }
    fn prepare_pairs(
        &self,
        s: &mut Scratch,
        input: &[f64],
        count: usize,
    ) -> Result<(u64, u32), String> {
        s.stream
            .memcpy_htod(input, &mut s.planes.slice_mut(..input.len()))
            .map_err(|e| e.to_string())?;
        s.stream
            .memcpy_htod(&[0_u32], &mut s.escaped)
            .map_err(|e| e.to_string())?;
        let clear = vec![0_u32; count];
        s.stream
            .memcpy_htod(&clear, &mut s.escape.slice_mut(..count))
            .map_err(|e| e.to_string())?;
        let p = i32::try_from(count).map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.prune);
        launch.arg(&s.planes).arg(&p).arg(&mut s.active);
        // SAFETY: p is bounded by 4096 and every plane/pair index is checked in
        // the kernel; the owning scratch outlives all stream work/readbacks.
        unsafe { launch.launch(config(count * count)) }.map_err(|e| e.to_string())?;
        let pairs = (count * count) as u32;
        let pair_blocks = pairs.div_ceil(BLOCK as u32);
        let mut launch = s.stream.launch_builder(&self.pair_scan);
        launch.arg(&mut s.active).arg(&pairs).arg(&mut s.pair_sums);
        // SAFETY: every block scans <=256 initialized weights; sums and prefixes
        // have one slot per checked pair/block. The padded tail is zero.
        unsafe { launch.launch(config(count * count)) }.map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.pair_offsets);
        launch
            .arg(&s.pair_sums)
            .arg(&pair_blocks)
            .arg(&mut s.pair_offsets);
        // SAFETY: one thread emits <=65536 block prefixes plus the total.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (1, 1, 1),
                block_dim: (1, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        let surviving = s
            .stream
            .clone_dtoh(
                &s.pair_offsets
                    .slice(pair_blocks as usize..pair_blocks as usize + 1),
            )
            .map_err(|e| e.to_string())?[0];
        Ok((surviving, pair_blocks))
    }

    fn dispatch_tile(
        &self,
        s: &mut Scratch,
        count: usize,
        pair_blocks: u32,
        start: u64,
        n: u32,
        collect_triples: bool,
    ) -> Result<(usize, bool), String> {
        let blocks = n.div_ceil(BLOCK as u32);
        let p = count as i32;
        let capture = u32::from(collect_triples);
        let mut launch = s.stream.launch_builder(&self.vertices);
        launch
            .arg(&s.planes)
            .arg(&s.active)
            .arg(&s.pair_offsets)
            .arg(&pair_blocks)
            .arg(&p)
            .arg(&start)
            .arg(&n)
            .arg(&mut s.candidates)
            .arg(&mut s.ranks)
            .arg(&mut s.blocks)
            .arg(&mut s.escape)
            .arg(&mut s.escaped);
        // SAFETY: the tile is <= TILE; all lanes, triple indices, planes
        // and shared-prefix entries are bounded by the checked dimensions.
        unsafe { launch.launch(config(n as usize)) }.map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.prefix);
        launch
            .arg(&mut s.blocks)
            .arg(&blocks)
            .arg(&s.escaped)
            .arg(&mut s.total);
        // SAFETY: exactly one thread prefixes <= TILE / BLOCK counts.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (1, 1, 1),
                block_dim: (1, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.compact);
        launch
            .arg(&s.candidates)
            .arg(&s.ranks)
            .arg(&s.blocks)
            .arg(&n)
            .arg(&start)
            .arg(&mut s.output)
            .arg(&mut s.ordinals)
            .arg(&s.active)
            .arg(&s.pair_offsets)
            .arg(&pair_blocks)
            .arg(&p)
            .arg(&capture);
        // SAFETY: the deterministic prefix places each valid lane in one
        // unique output entry, and their count never exceeds TILE.
        unsafe { launch.launch(config(n as usize)) }.map_err(|e| e.to_string())?;
        let metadata = s.stream.clone_dtoh(&s.total).map_err(|e| e.to_string())?;
        let total = metadata[0] as usize;
        if total > n as usize {
            return Err("CUDA solid compaction overflow".into());
        }
        let escaped = metadata[1];
        Ok((total, escaped != 0))
    }
}

fn read_candidates(
    s: &Scratch,
    total: usize,
    count: usize,
    collect_triples: bool,
    acc: &mut VertexAccumulator,
    captured: &mut Vec<([usize; 3], DVec3)>,
    work: &mut Work,
) -> Result<(), String> {
    let output = s
        .stream
        .clone_dtoh(&s.output.slice(..total * 3))
        .map_err(|e| e.to_string())?;
    work.readback_bytes += (output.len() * 8) as u64;
    if collect_triples {
        let ids = s
            .stream
            .clone_dtoh(&s.ordinals.slice(..total))
            .map_err(|e| e.to_string())?;
        work.readback_bytes += (ids.len() * 8) as u64;
        for (v, id) in output.as_chunks::<3>().0.iter().zip(ids) {
            let triple = unrank(id, count);
            if triple[2] < count - 6 {
                if captured.len() == 2_000_000 {
                    return Err(
                        "CUDA solid triple capture exceeds two-million-entry host budget".into(),
                    );
                }
                captured.push((triple, DVec3::new(v[0], v[1], v[2])));
            }
        }
    } else {
        for v in output.as_chunks::<3>().0.iter() {
            acc.insert_if_new(DVec3::new(v[0], v[1], v[2]));
        }
    }
    Ok(())
}

fn read_escaping(
    s: &Scratch,
    real_planes: usize,
    work: &mut Work,
) -> Result<Option<Vec<usize>>, String> {
    let escaped = s.stream.clone_dtoh(&s.escaped).map_err(|e| e.to_string())?[0];
    work.readback_bytes += 4;
    let escaping = if escaped == 0 {
        None
    } else {
        let flags = if real_planes == 0 {
            Vec::new()
        } else {
            s.stream
                .clone_dtoh(&s.escape.slice(..real_planes))
                .map_err(|e| e.to_string())?
        };
        work.readback_bytes += (flags.len() * 4) as u64;
        Some(
            flags
                .iter()
                .enumerate()
                .filter_map(|(i, &flag)| (flag != 0).then_some(i))
                .collect(),
        )
    };
    Ok(escaping)
}

fn config(tasks: usize) -> LaunchConfig {
    LaunchConfig {
        grid_dim: ((tasks as u32).div_ceil(BLOCK as u32), 1, 1),
        block_dim: (BLOCK as u32, 1, 1),
        shared_mem_bytes: 0,
    }
}

// Same lexicographic rank inverse as the device; only compacted feasible
// triples are unranked on the host, never the O(planes^3) search space.
fn unrank(mut index: u64, count: usize) -> [usize; 3] {
    let choose3 = |n: usize| {
        if n < 3 {
            0
        } else {
            n as u64 * (n as u64 - 1) * (n as u64 - 2) / 6
        }
    };
    let choose2 = |n: usize| {
        if n < 2 {
            0
        } else {
            n as u64 * (n as u64 - 1) / 2
        }
    };
    let total = choose3(count);
    let (mut lo, mut hi) = (0, count - 2);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if total - choose3(count - mid) <= index {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = lo;
    index -= total - choose3(count - a);
    (lo, hi) = (a + 1, count - 1);
    let rest = choose2(count - a - 1);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if rest - choose2(count - mid) <= index {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let b = lo;
    [a, b, b + 1 + (index - (rest - choose2(count - b))) as usize]
}
