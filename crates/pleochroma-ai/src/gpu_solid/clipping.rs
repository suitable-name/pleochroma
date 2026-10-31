//! Independent tool-face clips on CUDA; polygon ownership stays on the host.
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};
use glam::DVec3;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub(super) struct Clipping {
    context: Arc<CudaContext>,
    faces: CudaFunction,
    offsets: CudaFunction,
    pack: CudaFunction,
    state: Mutex<State>,
    pub batches: AtomicU64,
    pub polygons: AtomicU64,
    pub cache_hits: AtomicU64,
    pub readback: AtomicU64,
    pub scratch_bytes: AtomicU64,
}

#[derive(Default)]
struct State {
    scratch: Option<Scratch>,
    cache: VecDeque<CachedFaces>,
}
struct CachedFaces {
    key: Vec<u64>,
    faces: Vec<Vec<DVec3>>,
}
struct Scratch {
    capacity: usize,
    stride: usize,
    stream: Arc<CudaStream>,
    planes: CudaSlice<f64>,
    seeds: CudaSlice<f64>,
    a: CudaSlice<f64>,
    b: CudaSlice<f64>,
    output: CudaSlice<f64>,
    counts: CudaSlice<u32>,
    offsets: CudaSlice<u32>,
    error: CudaSlice<u32>,
}

impl Scratch {
    fn new(context: &Arc<CudaContext>, capacity: usize) -> Result<Self, String> {
        let stream = context.new_stream().map_err(|e| e.to_string())?;
        let stride = capacity + 4;
        Ok(Self {
            capacity,
            stride,
            planes: stream
                .alloc_zeros(capacity * 4)
                .map_err(|e| e.to_string())?,
            seeds: stream
                .alloc_zeros(capacity * 12)
                .map_err(|e| e.to_string())?,
            a: stream
                .alloc_zeros(capacity * stride * 3)
                .map_err(|e| e.to_string())?,
            b: stream
                .alloc_zeros(capacity * stride * 3)
                .map_err(|e| e.to_string())?,
            output: stream
                .alloc_zeros(capacity * stride * 3)
                .map_err(|e| e.to_string())?,
            counts: stream.alloc_zeros(capacity).map_err(|e| e.to_string())?,
            offsets: stream
                .alloc_zeros(capacity + 1)
                .map_err(|e| e.to_string())?,
            error: stream.alloc_zeros(1).map_err(|e| e.to_string())?,
            stream,
        })
    }
}

impl Clipping {
    pub fn new(context: Arc<CudaContext>) -> Result<Self, String> {
        let (major, minor) = context.compute_capability().map_err(|e| e.to_string())?;
        let ptx = compile_ptx_with_opts(
            include_str!("clipping.cu"),
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
            faces: module
                .load_function("tool_faces")
                .map_err(|e| e.to_string())?,
            offsets: module
                .load_function("face_offsets")
                .map_err(|e| e.to_string())?,
            pack: module
                .load_function("pack_faces")
                .map_err(|e| e.to_string())?,
            context,
            state: Mutex::default(),
            batches: AtomicU64::new(0),
            polygons: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            readback: AtomicU64::new(0),
            scratch_bytes: AtomicU64::new(0),
        })
    }

    pub fn faces(
        &self,
        planes: &[(DVec3, f64)],
        centre: DVec3,
        radius: f64,
    ) -> Result<Vec<Vec<DVec3>>, String> {
        let count = planes.len();
        if count == 0 {
            return Ok(Vec::new());
        }
        if count > 1024 {
            return Err("CUDA tool-face clipping exceeds 1024-plane budget".into());
        }
        let input: Vec<_> = planes
            .iter()
            .flat_map(|&(n, m)| [n.x, n.y, n.z, m])
            .collect();
        let key: Vec<_> = input
            .iter()
            .copied()
            .chain([centre.x, centre.y, centre.z, radius])
            .map(f64::to_bits)
            .collect();
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if let Some(cached) = state.cache.iter().find(|entry| entry.key == key) {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(cached.faces.clone());
        }
        if state.scratch.as_ref().is_none_or(|s| s.capacity < count) {
            state.scratch = None;
            state.scratch = Some(Scratch::new(&self.context, count.next_power_of_two())?);
        }
        let s = state
            .scratch
            .as_mut()
            .ok_or("missing CUDA clipping scratch")?;
        self.scratch_bytes.fetch_max(
            (s.capacity * s.stride * 72 + s.capacity * 136 + 8) as u64,
            Ordering::Relaxed,
        );
        let seeds = face_seeds(planes, centre, radius);
        self.dispatch_faces(s, &input, &seeds, count)?;
        let faces = self.read_faces(s, count, radius)?;
        if state.cache.len() == 4 {
            state.cache.pop_front();
        }
        state.cache.push_back(CachedFaces {
            key,
            faces: faces.clone(),
        });
        Ok(faces)
    }
    fn dispatch_faces(
        &self,
        s: &mut Scratch,
        input: &[f64],
        seeds: &[f64],
        count: usize,
    ) -> Result<(), String> {
        s.stream
            .memcpy_htod(input, &mut s.planes.slice_mut(..input.len()))
            .map_err(|e| e.to_string())?;
        s.stream
            .memcpy_htod(seeds, &mut s.seeds.slice_mut(..seeds.len()))
            .map_err(|e| e.to_string())?;
        s.stream
            .memcpy_htod(&[0_u32], &mut s.error)
            .map_err(|e| e.to_string())?;
        let n = count as u32;
        let stride = s.stride as u32;
        let mut launch = s.stream.launch_builder(&self.faces);
        launch
            .arg(&s.planes)
            .arg(&s.seeds)
            .arg(&n)
            .arg(&stride)
            .arg(&mut s.a)
            .arg(&mut s.b)
            .arg(&mut s.counts)
            .arg(&mut s.error);
        // SAFETY: <=1024 independent polygons each own two stride-sized slots.
        // Every append checks that slot capacity; overflow returns an error.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (n.div_ceil(64), 1, 1),
                block_dim: (64, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.offsets);
        launch.arg(&s.counts).arg(&n).arg(&mut s.offsets);
        // SAFETY: one thread prefixes n initialized entries into n+1 slots.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (1, 1, 1),
                block_dim: (1, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        let mut launch = s.stream.launch_builder(&self.pack);
        launch
            .arg(&s.a)
            .arg(&s.counts)
            .arg(&s.offsets)
            .arg(&n)
            .arg(&stride)
            .arg(&mut s.output);
        // SAFETY: each face writes a distinct output range derived from its
        // checked vertex count; all ranges fit capacity*stride vertices.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (n, 1, 1),
                block_dim: (64, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn read_faces(
        &self,
        s: &Scratch,
        count: usize,
        radius: f64,
    ) -> Result<Vec<Vec<DVec3>>, String> {
        let error = s.stream.clone_dtoh(&s.error).map_err(|e| e.to_string())?[0];
        if error != 0 {
            return Err("CUDA tool-face clipping polygon capacity overflow".into());
        }
        let offsets = s
            .stream
            .clone_dtoh(&s.offsets.slice(..count + 1))
            .map_err(|e| e.to_string())?;
        let total = offsets[count] as usize;
        if total > count * s.stride {
            return Err("CUDA tool-face clipping compaction overflow".into());
        }
        let output = if total == 0 {
            Vec::new()
        } else {
            s.stream
                .clone_dtoh(&s.output.slice(..total * 3))
                .map_err(|e| e.to_string())?
        };
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.polygons.fetch_add(count as u64, Ordering::Relaxed);
        self.readback.fetch_add(
            ((count + 2) * 4 + output.len() * 8) as u64,
            Ordering::Relaxed,
        );
        let mut faces = Vec::new();
        for span in offsets.windows(2) {
            let ring: Vec<_> = output[span[0] as usize * 3..span[1] as usize * 3]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| DVec3::new(p[0], p[1], p[2]))
                .collect();
            if let Some(&p0) = ring.first() {
                let mut area = DVec3::ZERO;
                for w in ring[1..].windows(2) {
                    area += (w[0] - p0).cross(w[1] - p0);
                }
                // Preserve the production area test before clipping to stone.
                if (0.5 * area).length() > 1e-14 * radius * radius {
                    faces.push(ring);
                }
            }
        }
        Ok(faces)
    }
}

fn face_seeds(planes: &[(DVec3, f64)], centre: DVec3, radius: f64) -> Vec<f64> {
    let count = planes.len();
    // Exact production seed arithmetic. Four seed corners define one face;
    // all O(tool_planes^2 * polygon_vertices) clipping runs on the device.
    let mut seeds = Vec::with_capacity(count * 12);
    for &(normal, offset) in planes {
        let u = normal.any_orthonormal_vector();
        let v = normal.cross(u);
        let foot = centre + normal * (offset - normal.dot(centre));
        let big = 4.0 * radius;
        for p in [
            foot - u * big - v * big,
            foot + u * big - v * big,
            foot + u * big + v * big,
            foot - u * big + v * big,
        ] {
            seeds.extend([p.x, p.y, p.z]);
        }
    }
    seeds
}
