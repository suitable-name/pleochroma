//! Opt-in validation of finalized XYZ, before the production reducer drops invalid
//! tuples. The default renderer neither allocates nor reads this staging buffer.

use super::{EwError, readback::map_read};

pub(super) struct SampleRange {
    pub pixel_offset: usize,
    pub sample_offset: u32,
    pub spp: u32,
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "wasm is single-threaded: the wgpu mapping future is not Send"
    )
)]
pub(super) async fn validate_chunk_samples(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    out_xyz: &wgpu::Buffer,
    rays: usize,
    range: SampleRange,
) -> Result<(), EwError> {
    let count = rays * 3;
    let bytes = (count * size_of::<f32>()) as wgpu::BufferAddress;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ew diagnostic raw XYZ staging"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ew diagnostic raw XYZ copy"),
    });
    encoder.copy_buffer_to_buffer(out_xyz, 0, &staging, 0, bytes);
    queue.submit(Some(encoder.finish()));
    let xyz = map_read(device, queue, &staging, count).await?;
    validate_samples(&xyz, &range)
}

fn validate_samples(xyz: &[f32], range: &SampleRange) -> Result<(), EwError> {
    for (index, value) in xyz.iter().enumerate() {
        // Inspect the loaded bits, without arithmetic or comparisons on a NaN.
        let bits = value.to_bits();
        if bits & 0x7f80_0000 == 0x7f80_0000 {
            let tuple = index / 3;
            let local_sample =
                u32::try_from(tuple % range.spp as usize).expect("a sample within spp fits in u32");
            return Err(EwError::NonFiniteSample {
                pixel: range.pixel_offset + tuple / range.spp as usize,
                sample: range.sample_offset.wrapping_add(local_sample),
                component: index % 3,
                bits,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EwError, SampleRange, validate_samples};

    #[test]
    fn raw_validation_preserves_finite_extremes_and_signed_zero() {
        let range = SampleRange {
            pixel_offset: 0,
            sample_offset: 0,
            spp: 2,
        };
        assert!(
            validate_samples(
                &[0.0, -0.0, f32::MAX, -f32::MAX, f32::from_bits(1), 1.0],
                &range,
            )
            .is_ok()
        );
    }

    #[test]
    fn every_nonfinite_component_reports_the_global_tuple_and_original_bits() {
        let range = SampleRange {
            pixel_offset: 40,
            sample_offset: 100,
            spp: 2,
        };
        for invalid in [0x7fc0_1234, 0x7f80_0000, 0xff80_0000] {
            for component in 0..3 {
                let mut xyz = [1.0; 12];
                xyz[9 + component] = f32::from_bits(invalid);
                assert!(matches!(
                    validate_samples(&xyz, &range),
                    Err(EwError::NonFiniteSample {
                        pixel: 41,
                        sample: 101,
                        component: actual_component,
                        bits,
                    }) if actual_component == component && bits == invalid
                ));
            }
        }
    }
}
