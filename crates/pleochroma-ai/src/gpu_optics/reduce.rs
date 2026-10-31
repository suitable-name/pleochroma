//! Ordered reductions copied from metrics/{evaluate,doubling,accumulate,tone}.rs.
use super::{OpticalError, lighting::ExitLighting, tone::ToneAccumulator};
use glam::Vec3;
use pleochroma::{color::metrics::GemOpticalMetrics, optics::materials::GemMaterial};

pub(super) struct PoseResult {
    pub metrics: GemOpticalMetrics,
    pub tone: Option<ToneAccumulator>,
}

pub(super) fn reduce(
    records: &[f32],
    material: &GemMaterial,
    observer: Vec3,
    lighting: &ExitLighting<'_>,
    half_width: f32,
    with_tone: bool,
    modes: usize,
) -> Result<PoseResult, OpticalError> {
    let mut total = 0u32;
    let (mut windowed, mut extinct, mut returned) = (0.0f32, 0.0f32, 0.0f32);
    let mut tone = if material.absorption.is_pleochroic {
        ToneAccumulator::with_principal_paths(half_width)
    } else {
        ToneAccumulator::new(half_width)
    };
    let (mut doubling_angle, mut doubling_weight) = (0.0f32, 0.0f32);
    for ray in records.chunks_exact(12 * modes) {
        if ray[0] == 0.0 {
            continue;
        }
        total += 1;
        let mut pair = [None, None];
        for (mode, record) in ray.as_chunks::<12>().0.iter().enumerate() {
            if record.iter().any(|v| !v.is_finite()) {
                return Err(OpticalError::Runtime("non-finite CUDA ray record".into()));
            }
            if with_tone {
                tone.count_total();
            }
            let share = 1.0 / modes as f32;
            match record[0] as u32 {
                1 => {}
                2 => windowed += share,
                3 => extinct += share,
                4 => {
                    let dir = Vec3::from_slice(&record[6..9]);
                    if dir.dot(observer) > lighting.head_shadow_cos()
                        || !lighting.is_illuminated(dir)
                    {
                        extinct += share;
                    } else {
                        returned += share;
                        pair[mode] = Some(record);
                        if with_tone {
                            tone.offer_principal(
                                share * record[1],
                                record[2],
                                [record[3], record[4], record[5]],
                            );
                        }
                    }
                }
                _ => return Err(OpticalError::Runtime("invalid CUDA ray fate".into())),
            }
        }
        if let [Some(o), Some(e)] = pair {
            if o[9] == e[9] {
                let od = Vec3::from_slice(&o[6..9]);
                let ed = Vec3::from_slice(&e[6..9]);
                let separation = od.cross(ed).length().atan2(od.dot(ed)).to_degrees() * 60.0;
                let weight = o[1] * e[1] * o[10].max(0.0) * e[10].max(0.0);
                doubling_angle = separation.mul_add(weight, doubling_angle);
                doubling_weight += weight;
            }
        }
    }
    let n = total.max(1) as f32;
    let metrics = GemOpticalMetrics {
        brilliance_pct: (returned / n * 100.0).clamp(0.0, 100.0),
        extinction_pct: (extinct / n * 100.0).clamp(0.0, 100.0),
        windowing_pct: (windowed / n * 100.0).clamp(0.0, 100.0),
        fire_index: 25.0,
        scintillation_pct: 75.0,
        doubling_arcmin: if doubling_weight > 0.0 {
            doubling_angle / doubling_weight
        } else {
            0.0
        },
    };
    Ok(PoseResult {
        metrics,
        tone: with_tone.then_some(tone),
    })
}
