//! Output, fixtures and parity budgets for the standalone optical probe.
mod config;
mod runner;
pub use config::{Config, HELP};
pub use runner::{CaseSpec, run_case};

use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{
        GpuFacetPlane, StoneGeometry,
        cuts::StandardGemCuts,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::materials::GemMaterial,
    render_setup::measure_model_width,
};
use pleochroma_ai::gpu_optics::{OpticalDiagnostics, ScoredOptics};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct Sink {
    writer: Box<dyn Write>,
    origin: Instant,
    origin_unix_ms: u64,
}

fn unix_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    u64::try_from(elapsed.as_millis()).map_err(|e| e.to_string())
}

impl Sink {
    pub fn open(path: Option<&Path>) -> Result<Self, String> {
        let writer: Box<dyn Write> = match path {
            Some(path) => Box::new(BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|error| format!("create new {}: {error}", path.display()))?,
            )),
            None => Box::new(BufWriter::new(std::io::stdout())),
        };
        Ok(Self {
            writer,
            origin: Instant::now(),
            origin_unix_ms: unix_ms()?,
        })
    }

    pub const fn origin_unix_ms(&self) -> u64 {
        self.origin_unix_ms
    }

    pub fn event(&mut self, kind: &str, mut value: Value) -> Result<(), String> {
        let fields = value
            .as_object_mut()
            .ok_or("event payload must be an object")?;
        fields.insert("event".into(), kind.into());
        fields.insert("timestamp_unix_ms".into(), unix_ms()?.into());
        fields.insert(
            "elapsed_seconds".into(),
            self.origin.elapsed().as_secs_f64().into(),
        );
        serde_json::to_writer(&mut self.writer, &value).map_err(|e| e.to_string())?;
        self.writer.write_all(b"\n").map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())
    }
}

pub struct Fixture {
    name: &'static str,
    planes: Vec<GpuFacetPlane>,
    pub material: GemMaterial,
    pub half_width: f32,
}

impl Fixture {
    pub fn round() -> Result<Self, String> {
        Self::new(
            "standard_round_brilliant/sapphire",
            StandardGemCuts::standard_round_brilliant(),
            GemMaterial::sapphire(),
        )
    }

    pub fn emerald() -> Result<Self, String> {
        Self::new(
            "emerald_cut/diamond",
            StandardGemCuts::emerald_cut(),
            GemMaterial::diamond(),
        )
    }

    fn new(
        name: &'static str,
        planes: Vec<GpuFacetPlane>,
        material: GemMaterial,
    ) -> Result<Self, String> {
        let exact: Vec<_> = planes
            .iter()
            .map(|plane| {
                (
                    glam::DVec3::new(
                        f64::from(plane.normal[0]),
                        f64::from(plane.normal[1]),
                        f64::from(plane.normal[2]),
                    ),
                    -f64::from(plane.d),
                )
            })
            .collect();
        if !matches!(build_solid_mesh(&exact), SolidStatus::Closed(_)) {
            return Err(format!("builtin fixture is not a closed solid: {name}"));
        }
        let width = measure_model_width(&planes)
            .filter(|width| width.is_finite() && *width > 0.0)
            .ok_or_else(|| format!("fixture width is not measurable: {name}"))?;
        let half_width = (width * 0.5) as f32;
        if !half_width.is_finite() || half_width <= 0.0 {
            return Err(format!("invalid quantized fan half-width: {name}"));
        }
        Ok(Self {
            name,
            planes,
            material,
            half_width,
        })
    }

    pub fn stone(&self) -> FinishedStone<'_> {
        FinishedStone::polished(StoneGeometry::planes_only(&self.planes))
    }

    pub fn description(&self) -> Value {
        json!({"name": self.name, "planes": self.planes.len(),
            "material": self.material.name, "half_width": self.half_width, "closed": true})
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Score {
    optical_percentages: [f32; 3],
    lch: [f32; 3],
    srgb: [u8; 3],
    mean_path_units: f32,
    returned_fraction: f32,
    poses: usize,
}

impl From<&ScoredOptics> for Score {
    fn from(score: &ScoredOptics) -> Self {
        Self {
            optical_percentages: [
                score.components.windowing_pct,
                score.components.extinction_pct,
                score.components.tilt_brilliance_pct,
            ],
            lch: [score.tone.l_star, score.tone.chroma, score.tone.hue_deg],
            srgb: score.tone.srgb,
            mean_path_units: score.tone.mean_path_units,
            returned_fraction: score.tone.returned_fraction,
            poses: score.poses,
        }
    }
}

impl Score {
    pub fn same_bits(&self, other: &Self) -> bool {
        self.optical_percentages.map(f32::to_bits) == other.optical_percentages.map(f32::to_bits)
            && self.lch.map(f32::to_bits) == other.lch.map(f32::to_bits)
            && self.srgb == other.srgb
            && self.mean_path_units.to_bits() == other.mean_path_units.to_bits()
            && self.returned_fraction.to_bits() == other.returned_fraction.to_bits()
            && self.poses == other.poses
    }

    pub fn finite(&self) -> bool {
        self.optical_percentages
            .iter()
            .chain(&self.lch)
            .chain([&self.mean_path_units, &self.returned_fraction])
            .all(|v| v.is_finite())
            && (0.0..=360.0).contains(&self.lch[2])
            && self.poses == 1
    }
}

#[derive(Default, Serialize)]
pub struct Agreement {
    compared: usize,
    bit_identical: usize,
    max_lc_absolute_error: f32,
    max_circular_hue_error: f32,
    max_path_absolute_error: f32,
    max_srgb_code_error: u8,
}

impl Agreement {
    pub fn observe(&mut self, reference: &Score, actual: &Score) -> Result<(), String> {
        if !reference.finite() || !actual.finite() {
            return Err("nonfinite or invalid Fast score".into());
        }
        let lc = (actual.lch[0] - reference.lch[0])
            .abs()
            .max((actual.lch[1] - reference.lch[1]).abs());
        let hue = (actual.lch[2] - reference.lch[2]).abs();
        let hue = hue.min(360.0 - hue);
        let path = (actual.mean_path_units - reference.mean_path_units).abs();
        let rgb = actual
            .srgb
            .iter()
            .zip(reference.srgb)
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap_or(0);
        let exact = actual.optical_percentages.map(f32::to_bits)
            == reference.optical_percentages.map(f32::to_bits)
            && actual.returned_fraction.to_bits() == reference.returned_fraction.to_bits()
            && actual.poses == reference.poses;
        if !exact
            || lc > 0.01
            || hue > 0.05
            || rgb > 1
            || path > 1e-5 * reference.mean_path_units.abs().max(1.0)
        {
            return Err(format!(
                "same-GPU score disagreement: reference={reference:?}, actual={actual:?}"
            ));
        }
        self.compared += 1;
        self.bit_identical += usize::from(actual.same_bits(reference));
        self.max_lc_absolute_error = self.max_lc_absolute_error.max(lc);
        self.max_circular_hue_error = self.max_circular_hue_error.max(hue);
        self.max_path_absolute_error = self.max_path_absolute_error.max(path);
        self.max_srgb_code_error = self.max_srgb_code_error.max(rgb);
        Ok(())
    }
}

pub fn diagnostics_delta(before: &OpticalDiagnostics, after: &OpticalDiagnostics) -> Value {
    let completed = after.completed_requests - before.completed_requests;
    let batches = after.fast_batches - before.fast_batches;
    let mean = if batches == 0 {
        0.0
    } else {
        completed as f64 / batches as f64
    };
    json!({
        "completed_requests": completed, "fast_batches": batches,
        "mean_fast_batch_candidates": mean,
        "mean_batch_fill_fraction": mean / after.capacity.max_fast_candidates.max(1) as f64,
        "max_fast_batch_candidates_lifetime": after.max_fast_batch_candidates,
        "launches": after.launches - before.launches, "rays": after.rays - before.rays,
        "spectral_exponentials": after.spectral_exponentials - before.spectral_exponentials,
        "service_wall_micros": after.service_wall_micros - before.service_wall_micros,
        "queued_requests_end": after.queued_requests, "active_requests_end": after.active_requests,
        "first_error": after.first_error
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn score() -> Score {
        Score {
            optical_percentages: [10.0, 20.0, 70.0],
            lch: [50.0, 5.0, 0.01],
            srgb: [20, 30, 40],
            mean_path_units: 1.0,
            returned_fraction: 0.7,
            poses: 1,
        }
    }
    #[test]
    fn continuous_budget_never_relaxes_discrete_ray_counts() {
        let reference = score();
        let mut actual = reference.clone();
        actual.lch[0] += 0.005;
        actual.lch[2] = 359.99;
        assert!(Agreement::default().observe(&reference, &actual).is_ok());
        actual.optical_percentages[0] = actual.optical_percentages[0].next_up();
        assert!(Agreement::default().observe(&reference, &actual).is_err());
        actual = reference.clone();
        actual.mean_path_units = f32::NAN;
        assert!(Agreement::default().observe(&reference, &actual).is_err());
    }
    #[test]
    fn per_case_mean_uses_deltas_and_maximum_is_lifetime_scoped() {
        let before = OpticalDiagnostics {
            completed_requests: 12,
            fast_batches: 3,
            max_fast_batch_candidates: 8,
            ..OpticalDiagnostics::default()
        };
        let after = OpticalDiagnostics {
            completed_requests: 20,
            fast_batches: 7,
            max_fast_batch_candidates: 8,
            ..OpticalDiagnostics::default()
        };
        let result = diagnostics_delta(&before, &after);
        assert_eq!(result["mean_fast_batch_candidates"], 2.0);
        assert_eq!(result["max_fast_batch_candidates_lifetime"], 8);
    }
}
