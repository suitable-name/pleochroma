//! Deterministic linear-sRGB environment lookups; no transport samples are drawn.

use std::{borrow::Cow, sync::Arc};

use glam::Vec3;

use crate::{
    optics::{
        raytracer::{
            DEFAULT_HEAD_SHADOW_COSINES, EnvironmentSource, LightingPreset,
            environment::{
                TrayMirror, TrayShadowMap, hdr_rgb_at_exposure, head_shadow_cosines,
                sample_studio_environment_with_rig_shadow_tray,
                sample_studio_environment_with_rig_spec_power_shadow_tray,
            },
        },
        studio_rig::StudioRig,
    },
    renderer::env_map::{EnvRotation, EnvironmentMap},
};

use super::tint::{illuminant_rgb, integrate_rgb};

enum Source<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        rig: Box<StudioRig>,
        shadow: [f32; 2],
        rgb: Vec3,
        backdrop: f32,
    },
    Aset {
        table: Box<[Vec3; 1024]>,
        rgb: Vec3,
        backdrop: f32,
    },
    /// An HDR map at the scene's exposure, turned by `rotation` (`EnvironmentSource::HdrMap`).
    Hdr {
        map: &'a EnvironmentMap,
        exposure: f32,
        rotation: EnvRotation,
    },
    OwnedHdr {
        map: Arc<EnvironmentMap>,
        exposure: f32,
        rotation: EnvRotation,
    },
    #[cfg(test)]
    Constant(Vec3),
}

/// Prepared environment radiance, expressed in linear sRGB.
pub struct WhittedEnv<'a> {
    source: Source<'a>,
    tray_shadow: Option<Cow<'a, TrayShadowMap>>,
    /// The glossy tray under the stone, when the scene asks for one
    /// (`EnvironmentSource::tray_mirror`). The chain reads it at every downward escape.
    mirror: Option<TrayMirror>,
}

/// Borrowed prepared data: GPU uploads reuse the CPU tables and rig rather than
/// independently integrating spectra or approximating a directional environment.
#[cfg(feature = "gpu")]
pub enum WhittedGpuEnv<'a> {
    Studio {
        preset: LightingPreset,
        exposure: f32,
        rig: &'a StudioRig,
        shadow: [f32; 2],
        rgb: Vec3,
        backdrop: f32,
        /// The spectral map whose RGB texels follow the studio dummy texel on the device.
        tray_shadow: Option<&'a TrayShadowMap>,
        /// The glossy tray (`wt_floor` on the device); `None` for a matte tray.
        tray_mirror: Option<TrayMirror>,
    },
    Aset {
        table: &'a [Vec3; 1024],
        rgb: Vec3,
        backdrop: f32,
    },
    /// The map's texels (uploaded as they are), the exposure the kernel scales them by and the
    /// map's yaw (the kernel rotates the lookup direction into the map frame).
    Hdr {
        map: &'a EnvironmentMap,
        exposure: f32,
        rotation: EnvRotation,
    },
    #[cfg(test)]
    Constant(Vec3),
}

impl<'a> WhittedEnv<'a> {
    #[cfg(feature = "gpu")]
    pub(crate) fn gpu_data(&self) -> WhittedGpuEnv<'_> {
        match &self.source {
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => WhittedGpuEnv::Studio {
                preset: *preset,
                exposure: *exposure,
                rig,
                shadow: *shadow,
                rgb: *rgb,
                backdrop: *backdrop,
                tray_shadow: self.tray_shadow.as_deref(),
                tray_mirror: self.mirror,
            },
            Source::Aset {
                table,
                rgb,
                backdrop,
            } => WhittedGpuEnv::Aset {
                table,
                rgb: *rgb,
                backdrop: *backdrop,
            },
            Source::Hdr {
                map,
                exposure,
                rotation,
            } => WhittedGpuEnv::Hdr {
                map,
                exposure: *exposure,
                rotation: *rotation,
            },
            Source::OwnedHdr {
                map,
                exposure,
                rotation,
            } => WhittedGpuEnv::Hdr {
                map,
                exposure: *exposure,
                rotation: *rotation,
            },
            #[cfg(test)]
            Source::Constant(rgb) => WhittedGpuEnv::Constant(*rgb),
        }
    }

    /// The owning handle of an HDR map this sampler holds, when it holds one.
    ///
    /// Only an owned map has an identity a GPU upload cache can pin: a borrowed
    /// `Source::Hdr` has no owner, so it stays uncacheable.
    #[cfg(feature = "gpu")]
    #[must_use]
    pub(crate) const fn shared_hdr(&self) -> Option<&Arc<EnvironmentMap>> {
        match &self.source {
            Source::OwnedHdr { map, .. } => Some(map),
            _ => None,
        }
    }

    pub(super) fn new(environment: EnvironmentSource<'a>) -> Self {
        let source = match environment {
            EnvironmentSource::HdrMap {
                map,
                exposure,
                rotation,
            } => Source::Hdr {
                map,
                exposure,
                rotation,
            },
            EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                head_shadow_deg,
                ..
            } => {
                let rig = StudioRig::new(light_yaw, light_pitch);
                let rgb = illuminant_rgb(environment);
                if preset == LightingPreset::Aset {
                    let table = Box::new(std::array::from_fn(|row| {
                        #[expect(clippy::cast_precision_loss, reason = "table index is below 1024")]
                        let y = (row as f32 / 1023.0).mul_add(2.0, -1.0);
                        // Operation order mirrors the WGSL twin in `00_bindings.wgsl`; the square
                        // is a separate binding so clippy does not suggest a fused multiply-add.
                        let square = y * y;
                        let dir = Vec3::new((1.0 - square).max(0.0).sqrt(), y, 0.0);
                        // The lit tray (`backdrop`, 0.0 = As Lit) is part of the table, so
                        // the ASET surround and its windows read the same value.
                        integrate_rgb(|wavelength| {
                            sample_studio_environment_with_rig_shadow_tray(
                                dir,
                                wavelength,
                                preset,
                                exposure,
                                &rig,
                                Vec3::ZERO,
                                DEFAULT_HEAD_SHADOW_COSINES,
                                backdrop,
                            )
                        })
                    }));
                    Source::Aset {
                        table,
                        rgb,
                        backdrop,
                    }
                } else {
                    Source::Studio {
                        preset,
                        exposure,
                        rig: Box::new(rig),
                        shadow: head_shadow_cosines(head_shadow_deg),
                        rgb,
                        backdrop,
                    }
                }
            }
        };
        // Only a Studio rig carries the mirror, as on the device: ASET and HDR drop it.
        let mirror = if matches!(source, Source::Studio { .. }) {
            environment.tray_mirror()
        } else {
            None
        };
        Self {
            source,
            tray_shadow: environment.tray_shadow().map(Cow::Borrowed),
            mirror,
        }
    }

    /// The glossy tray this sampler's escapes may bounce off, `None` when the tray is matte.
    ///
    /// The CPU chain (`chain::resolve_escape`) reads it; the WGSL twin of the floor split is
    /// `wt_floor_escape` (`50_chain.wgsl`), which calls `wt_floor`, the twin of `TrayMirror::hit`.
    /// The GPU upload takes the field directly in `gpu_data`.
    #[must_use]
    pub(crate) const fn mirror(&self) -> Option<TrayMirror> {
        self.mirror
    }

    /// The same sampler with `mirror` as its glossy tray (tests only).
    #[cfg(test)]
    #[must_use]
    pub(super) const fn with_mirror(mut self, mirror: Option<TrayMirror>) -> Self {
        self.mirror = mirror;
        self
    }

    /// Radiance reaching a leaf in `dir`, with the pixel's direction toward its eye.
    ///
    /// An HDR map's leaf is its bilinear texel at the scene's exposure
    /// ([`hdr_rgb_at_exposure`]) looked up at the direction rotated into the map's frame; the
    /// WGSL twin is `wt_hdr_at_exposure(wt_hdr(dir))`.
    #[must_use]
    pub fn leaf(&self, dir: Vec3, observer: Vec3) -> Vec3 {
        match &self.source {
            Source::Hdr {
                map,
                exposure,
                rotation,
            } => Vec3::from_array(hdr_rgb_at_exposure(
                map.radiance_rgb_rotated(dir, *rotation),
                *exposure,
            )),
            Source::OwnedHdr {
                map,
                exposure,
                rotation,
            } => Vec3::from_array(hdr_rgb_at_exposure(
                map.radiance_rgb_rotated(dir, *rotation),
                *exposure,
            )),
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => {
                // One arithmetic path for every head-shadow setting and for the lit
                // backdrop tray (`backdrop` = its radiance at exposure 1, 0.0 = As Lit):
                // a primary miss, a window and a reflection all call it, so they agree.
                let geometry = sample_studio_environment_with_rig_spec_power_shadow_tray(
                    dir, *preset, *exposure, rig, observer, 1.0, *shadow, *backdrop,
                );
                *rgb * geometry
            }
            Source::Aset { table, .. } => {
                let position = (dir.y.clamp(-1.0, 1.0) + 1.0) * 511.5;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped table coordinate is in 0..=1023"
                )]
                let lower = position as usize;
                #[expect(clippy::cast_precision_loss, reason = "table index is below 1024")]
                let fraction = position - lower as f32;
                table[lower].lerp(table[(lower + 1).min(1023)], fraction)
            }
            #[cfg(test)]
            Source::Constant(rgb) => *rgb,
        }
    }

    /// Radiance at a leaf escaping from `origin`. A downward studio ray resolves the
    /// paper at its landing point and lights each channel with the corresponding tray.
    #[must_use]
    pub fn leaf_at(&self, origin: Vec3, dir: Vec3, observer: Vec3) -> Vec3 {
        if let Source::Studio {
            preset,
            exposure,
            rig,
            shadow,
            rgb,
            backdrop,
        } = &self.source
            && let Some(factors) = self
                .tray_shadow
                .as_deref()
                .and_then(|map| map.factor_rgb_for(origin, dir))
        {
            return *rgb
                * Vec3::from_array(factors.map(|factor| {
                    sample_studio_environment_with_rig_spec_power_shadow_tray(
                        dir,
                        *preset,
                        *exposure,
                        rig,
                        observer,
                        1.0,
                        *shadow,
                        (*backdrop * factor).max(f32::MIN_POSITIVE),
                    )
                }));
        }
        self.leaf(dir, observer)
    }

    /// Detaches a prepared environment from its scene for long-lived frame caches.
    /// An HDR map is copied once; no per-tap or per-chunk copy occurs.
    #[must_use]
    pub fn into_owned(self) -> WhittedEnv<'static> {
        self.into_owned_with_hdr(None)
    }

    /// Reuses the scene's existing HDR allocation when it is the exact map this
    /// sampler borrows. A mismatching Arc cannot silently replace the lighting.
    #[must_use]
    pub fn into_owned_with_hdr(self, shared: Option<Arc<EnvironmentMap>>) -> WhittedEnv<'static> {
        let source = match self.source {
            Source::Hdr {
                map,
                exposure,
                rotation,
            } => Source::OwnedHdr {
                map: shared
                    .filter(|candidate| std::ptr::eq(map, candidate.as_ref()))
                    .unwrap_or_else(|| Arc::new(map.clone())),
                exposure,
                rotation,
            },
            Source::OwnedHdr {
                map,
                exposure,
                rotation,
            } => Source::OwnedHdr {
                map,
                exposure,
                rotation,
            },
            Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => Source::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            },
            Source::Aset {
                table,
                rgb,
                backdrop,
            } => Source::Aset {
                table,
                rgb,
                backdrop,
            },
            #[cfg(test)]
            Source::Constant(rgb) => Source::Constant(rgb),
        };
        WhittedEnv {
            source,
            tray_shadow: self.tray_shadow.map(|map| Cow::Owned(map.into_owned())),
            mirror: self.mirror,
        }
    }

    #[cfg(test)]
    pub(super) const fn for_tests(rgb: Vec3) -> Self {
        Self {
            source: Source::Constant(rgb),
            tray_shadow: None,
            mirror: None,
        }
    }
}

/// Tests of the HDR map yaw through the leaf lookup.
#[cfg(test)]
mod tests {
    use super::*;

    /// A map whose radiance changes with the azimuth and the elevation.
    fn map() -> EnvironmentMap {
        let (width, height) = (12, 6);
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = x as f32 / width as f32;
                    let v = y as f32 / height as f32;
                    [
                        (1.5 * u).mul_add(u, 0.2),
                        0.9f32.mul_add(v, 0.2),
                        u.mul_add(v, 0.05) * 2.5,
                    ]
                })
            })
            .collect();
        EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
    }

    /// `map` with every row moved `columns` texels toward larger `u` (wrapping).
    fn shifted(map: &EnvironmentMap, columns: usize) -> EnvironmentMap {
        let (width, height) = (map.width(), map.height());
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (y, x)))
            .map(|(y, x)| map.texels()[y * width + (x + width - columns) % width])
            .collect();
        EnvironmentMap::from_rgb(width, height, pixels).expect("self-consistent by construction")
    }

    /// Directions over the sphere, with both poles and the seam.
    fn directions() -> Vec<Vec3> {
        let mut dirs = vec![Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
        for i in 0..13u8 {
            for j in 0..7u8 {
                let phi = f32::from(i).mul_add(std::f32::consts::TAU / 13.0, 0.05);
                let theta = (f32::from(j) + 0.5) * (std::f32::consts::PI / 7.0);
                let (st, ct) = theta.sin_cos();
                let (sp, cp) = phi.sin_cos();
                dirs.push(Vec3::new(st * sp, ct, st * cp));
            }
        }
        dirs
    }

    fn bits(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    #[test]
    fn a_turned_leaf_reads_the_map_in_its_own_frame_and_keeps_it_when_owned() {
        let map = map();
        let rotation = EnvRotation::from_yaw(0.8);
        let turned_source = EnvironmentSource::hdr_at(&map, 2.0).with_hdr_rotation(rotation);
        let turned = WhittedEnv::new(turned_source);
        let owned = WhittedEnv::new(turned_source).into_owned();
        let plain = WhittedEnv::new(EnvironmentSource::hdr_at(&map, 2.0));
        for dir in directions() {
            let expected = bits(plain.leaf(rotation.to_map(dir), Vec3::ZERO));
            assert_eq!(bits(turned.leaf(dir, Vec3::ZERO)), expected);
            assert_eq!(bits(owned.leaf(dir, Vec3::ZERO)), expected);
        }
    }

    #[test]
    fn an_unturned_leaf_is_the_plain_leaf_bit_for_bit() {
        let map = map();
        let plain = WhittedEnv::new(EnvironmentSource::hdr_at(&map, 0.7));
        for yaw in [0.0, -0.0, std::f32::consts::TAU, f32::NAN] {
            let source =
                EnvironmentSource::hdr_at(&map, 0.7).with_hdr_rotation(EnvRotation::from_yaw(yaw));
            let unturned = WhittedEnv::new(source);
            for dir in directions() {
                assert_eq!(
                    bits(unturned.leaf(dir, Vec3::ZERO)),
                    bits(plain.leaf(dir, Vec3::ZERO))
                );
            }
        }
    }

    /// A map turned by whole texel columns is the map with its columns shifted.
    ///
    /// Away from the poles only: at a pole the azimuth is undefined, so the lookup reads the
    /// column `atan2(0, 0)` names in whichever map it holds. The turn leaves the pole where it
    /// is while the shift moves that column, and the two lookups read different texels there.
    #[test]
    fn a_whole_column_turn_equals_shifted_columns() {
        let map = map();
        let columns = 3;
        let yaw = std::f32::consts::TAU * columns as f32 / map.width() as f32;
        let turned = WhittedEnv::new(
            EnvironmentSource::hdr(&map).with_hdr_rotation(EnvRotation::from_yaw(yaw)),
        );
        let shifted_map = shifted(&map, columns);
        let moved = WhittedEnv::new(EnvironmentSource::hdr(&shifted_map));
        for dir in directions()
            .into_iter()
            .filter(|d| d.x != 0.0 || d.z != 0.0)
        {
            let (a, b) = (turned.leaf(dir, Vec3::ZERO), moved.leaf(dir, Vec3::ZERO));
            assert!(
                (a - b).abs().max_element() <= 2e-6 * (1.0 + b.abs().max_element()),
                "{dir}: {a} against {b}"
            );
        }
    }
}
