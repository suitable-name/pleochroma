//! GPU buffer readback for native and wasm32 targets.
//!
//! Every error path unmaps the staging buffer (cancelling a still-pending `map_async`),
//! so the next chunk's readback on the same buffer is valid again.

use super::EwError;

/// Copies the first `count` `f32`s out of the mapped staging buffer and unmaps it on every path.
fn read_mapped(buffer: &wgpu::Buffer, count: usize) -> Result<Vec<f32>, EwError> {
    let result = {
        match buffer.slice(..).get_mapped_range() {
            Err(e) => Err(EwError::DeviceLost(format!(
                "mapped buffer range could not be read: {e}"
            ))),
            Ok(data) => {
                let floats = bytemuck::cast_slice::<u8, f32>(&data);
                if floats.len() < count {
                    Err(EwError::DeviceLost(format!(
                        "mapped buffer too small: expected {count} floats, found {}",
                        floats.len()
                    )))
                } else {
                    Ok(floats[..count].to_vec())
                }
            }
        }
    };
    buffer.unmap();
    result
}

/// Reads `count` `f32`s from a mappable staging buffer.
///
/// On native: starts the async mapping and polls until it completes or the chunk deadline
/// set through `clock::set_readback_deadline_ms` passes, never sooner than
/// `compute::GPU_WAIT_TIMEOUT`.
/// On wasm32: awaits a `futures_channel::oneshot` channel triggered by the browser `mapAsync`
/// callback, raced against the chunk deadline set through `clock::set_readback_deadline_ms`.
///
/// On a timeout the result is [`EwError::ChunkTimeout`]; on every error the staging buffer
/// is unmapped.
///
/// # Errors
///
/// [`EwError::ChunkTimeout`] if the mapping does not complete in time, or
/// [`EwError::DeviceLost`] if polling or mapping the buffer fails.
#[cfg(not(target_arch = "wasm32"))]
pub async fn map_read(
    device: &wgpu::Device,
    _queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<f32>, EwError> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

    let wait = native_wait();
    let start = std::time::Instant::now();
    loop {
        match device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(wait.saturating_sub(start.elapsed())),
        }) {
            Ok(_) => {}
            // A slow GPU, not a lost one.
            Err(wgpu::PollError::Timeout) => {
                buffer.unmap();
                return Err(EwError::ChunkTimeout);
            }
            Err(e) => {
                buffer.unmap();
                return Err(EwError::DeviceLost(format!("device poll failed: {e}")));
            }
        }

        match rx.try_recv() {
            Ok(Ok(())) => break,
            Ok(Err(e)) => {
                buffer.unmap();
                return Err(EwError::DeviceLost(format!("buffer mapping failed: {e}")));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if start.elapsed() >= wait {
                    buffer.unmap();
                    return Err(EwError::ChunkTimeout);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                buffer.unmap();
                return Err(EwError::DeviceLost(
                    "readback channel disconnected unexpectedly".to_string(),
                ));
            }
        }
    }

    read_mapped(buffer, count)
}

/// How long a native readback waits: the chunk deadline the accumulation loop set
/// (`clock::chunk_deadline_ms`, ten seconds for a scene's first chunk), never less than
/// `compute::GPU_WAIT_TIMEOUT`. The deadline is process-wide and another thread's renderer
/// may have set it last; the floor keeps that from shortening this wait.
#[cfg(not(target_arch = "wasm32"))]
fn native_wait() -> std::time::Duration {
    let floor = crate::renderer::gpu::compute::GPU_WAIT_TIMEOUT;
    std::time::Duration::try_from_secs_f64(super::clock::readback_deadline_ms() / 1000.0)
        .map_or(floor, |deadline| deadline.max(floor))
}

/// Reads `count` `f32`s from a mappable staging buffer on wasm32.
///
/// # Errors
///
/// [`EwError::ChunkTimeout`] if the browser's `mapAsync` callback does not fire before the
/// chunk deadline, or [`EwError::DeviceLost`] if the mapping fails or its callback is dropped.
#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::future_not_send,
    reason = "wgpu web backend buffer mapping futures are !Send; wasm32 is single-threaded"
)]
pub async fn map_read(
    _device: &wgpu::Device,
    _queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<f32>, EwError> {
    use super::clock::{Raced, race_deadline, readback_deadline_ms};

    let (tx, rx) = futures_channel::oneshot::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

    match race_deadline(rx, readback_deadline_ms()).await {
        Raced::TimedOut => {
            buffer.unmap();
            return Err(EwError::ChunkTimeout);
        }
        Raced::Done(Err(_)) => {
            buffer.unmap();
            return Err(EwError::DeviceLost(
                "buffer-mapping callback was dropped before firing".to_string(),
            ));
        }
        Raced::Done(Ok(Err(e))) => {
            buffer.unmap();
            return Err(EwError::DeviceLost(format!(
                "wgpu buffer mapping failed: {e}"
            )));
        }
        Raced::Done(Ok(Ok(()))) => {}
    }

    read_mapped(buffer, count)
}
