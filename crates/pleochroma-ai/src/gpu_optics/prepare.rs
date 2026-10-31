//! Candidate packing; d-line conventions copied from metrics/{prepared,birefringent}.rs.
use super::{OpticalError, fan::FanGeometry};
use glam::{Mat3, Vec3};
use pleochroma::{
    color::{
        metrics::{FinishedStone, PROFILE_AZIMUTHS_DEG, camera_view_basis},
        principal_axes::PrincipalFrame,
    },
    geometry::tool::MAX_TOOL_PRIMITIVES,
    optics::{
        materials::{CrystalSystem, GemMaterial},
        raytracer::FacetFinish,
    },
};
use pleochroma_cut_core::optimize::ObjectiveFidelity;

pub(super) fn validate(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
) -> Result<(), OpticalError> {
    if stone.geom.planes.len() > 4096 {
        return Err(OpticalError::Unsupported(
            "more than 4096 facet planes".into(),
        ));
    }
    if stone.geom.tools.len() > MAX_TOOL_PRIMITIVES {
        return Err(OpticalError::Unsupported(format!(
            "more than {MAX_TOOL_PRIMITIVES} tools"
        )));
    }
    if stone
        .geom
        .planes
        .iter()
        .any(|p| !p.d.is_finite() || p.normal.iter().any(|v| !v.is_finite()))
    {
        return Err(OpticalError::InvalidInput("non-finite facet plane".into()));
    }
    for t in stone.geom.tools {
        t.validate()
            .map_err(|e| OpticalError::InvalidInput(e.to_string()))?;
    }
    if !material.dispersion.evaluate(589.3).is_finite()
        || !material.c_axis.is_finite()
        || !material.birefringence_delta.is_finite()
    {
        return Err(OpticalError::InvalidInput(
            "non-finite optical material".into(),
        ));
    }
    let tensor = &material.absorption;
    if tensor.o_ray.len() > 4096
        || tensor.e_ray.len() > 4096
        || tensor.beta_ray.as_ref().is_some_and(|b| b.len() > 4096)
    {
        return Err(OpticalError::Unsupported(
            "more than 4096 absorption bands per principal axis".into(),
        ));
    }
    Ok(())
}

pub(super) fn material_params(material: &GemMaterial) -> Vec<f32> {
    let no = material.dispersion.evaluate(589.3).max(1.1);
    let ne = material
        .extraordinary_index_at(589.3, material.dispersion.evaluate(589.3))
        .max(1.001);
    let c = material.c_axis.try_normalize().unwrap_or(Vec3::Y);
    let biaxial = material.biaxial_indicatrix(589.3);
    let anisotropic = material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4;
    let kind = if !anisotropic {
        0.0
    } else if biaxial.is_some() {
        2.0
    } else {
        1.0
    };
    let (na, nb, ng, axes) = biaxial.map_or((no, no, ne, Mat3::IDENTITY), |b| {
        (b.n_alpha, b.n_beta, b.n_gamma, b.axes)
    });
    let principal = PrincipalFrame::for_pleochroic(material);
    let frame = PrincipalFrame::of_material(material);
    let mut data = vec![kind, no, ne, na, nb, ng];
    data.extend(c.to_array());
    data.extend(axes.to_cols_array());
    data.extend(frame.axes().to_cols_array());
    data.push(if principal.is_some() { 1.0 } else { 0.0 });
    data.push(if biaxial.is_some() { 2.0 } else { 1.0 });
    data.push(material.extraordinary_index_at(589.3, no) - no);
    data
}

pub(super) fn finishes(stone: FinishedStone<'_>) -> Vec<u32> {
    if !stone.finishes.contains(&FacetFinish::Frosted) {
        return vec![0];
    }
    stone
        .finishes
        .iter()
        .map(|f| u32::from(*f == FacetFinish::Frosted))
        .collect()
}

/// Tilt-indexed order is the CPU components_from_profiles f64 reduction order.
pub(super) fn poses(fidelity: ObjectiveFidelity) -> Vec<(f32, f32)> {
    let mut out = vec![(0.0, 90.0f32.to_radians())];
    if matches!(fidelity, ObjectiveFidelity::Full) {
        for azimuth in PROFILE_AZIMUTHS_DEG {
            for tilt in -90_i32..=90 {
                let yaw = if tilt < 0 {
                    (azimuth + 180.0).to_radians()
                } else {
                    azimuth.to_radians()
                };
                out.push((yaw, ((90 - tilt.abs()) as f32).to_radians()));
            }
        }
    }
    out
}

pub(super) fn pose_data(poses: &[(f32, f32)], fan: FanGeometry, faceup_first: bool) -> Vec<f32> {
    let (standoff, lateral) = fan.cuda_parameters();
    let mut out = Vec::with_capacity(poses.len() * 12);
    for (index, &(yaw, pitch)) in poses.iter().enumerate() {
        let (forward, right, up) = camera_view_basis(yaw, pitch);
        out.extend(forward.to_array());
        out.extend(right.to_array());
        out.extend(up.to_array());
        out.push(standoff);
        out.push(lateral);
        out.push(if faceup_first && index == 0 { 1.0 } else { 0.0 });
    }
    out
}

pub(super) fn cells() -> Vec<f32> {
    let mut out = Vec::new();
    for ix in 0..18 {
        for iz in 0..18 {
            let u = ((ix as f32 + 0.5) / 18.0).mul_add(2.0, -1.0);
            let v = ((iz as f32 + 0.5) / 18.0).mul_add(2.0, -1.0);
            if v.mul_add(v, u * u) <= 0.70 {
                out.extend([u, v]);
            }
        }
    }
    out
}
