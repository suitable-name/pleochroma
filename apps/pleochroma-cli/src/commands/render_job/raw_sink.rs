//! The `PLRAW1` files of `--raw-out` and `--progress-raw`: the sink that receives a render's
//! linear image from the renderer and writes it with [`RawImage`].

use crate::raw_image::RawImage;
use pleochroma_cut::gui::render_jobs::headless::{LinearImage, LinearImageKind, RawTapSink};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Where `--raw-out` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RawTarget {
    /// One file: the finished picture of a `render`.
    File(PathBuf),
    /// A folder: one `NAME.plraw` for each frame a `tilt-video` renders, `NAME` being the
    /// stem of the frame's PNG.
    Folder(PathBuf),
}

/// The name of the `--progress-raw` file holding `samples_per_pixel` samples per pixel.
#[must_use]
pub(super) fn snapshot_file_name(samples_per_pixel: u32) -> String {
    format!("spp_{samples_per_pixel:06}.plraw")
}

/// Writes the linear images of a render as `PLRAW1` files.
///
/// A write that fails does not stop the render; the first failure is kept for
/// [`Self::take_error`], which the command reports once the render is over.
#[derive(Debug)]
pub(super) struct RawFileSink {
    /// Where the finished image of a frame goes (`--raw-out`).
    target: Option<RawTarget>,
    /// The folder of the power-of-two snapshots and the final image (`--progress-raw`).
    progress_dir: Option<PathBuf>,
    /// The first write error.
    first_error: Mutex<Option<String>>,
    /// How many files were written.
    written: AtomicUsize,
}

impl RawFileSink {
    /// A sink writing to `target` and/or `progress_dir`.
    #[must_use]
    pub(super) const fn new(target: Option<RawTarget>, progress_dir: Option<PathBuf>) -> Self {
        Self {
            target,
            progress_dir,
            first_error: Mutex::new(None),
            written: AtomicUsize::new(0),
        }
    }

    /// The first write error, if any write failed.
    pub(super) fn take_error(&self) -> Option<String> {
        self.first_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// How many files were written.
    #[must_use]
    pub(super) fn files_written(&self) -> usize {
        self.written.load(Ordering::Relaxed)
    }

    /// Remembers `message` unless an earlier failure is already kept.
    fn fail(&self, message: String) {
        let mut first = self
            .first_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if first.is_none() {
            *first = Some(message);
        }
    }

    /// The files `image` is written to.
    fn destinations(&self, image: &LinearImage<'_>) -> Vec<PathBuf> {
        let snapshot_name = snapshot_file_name(image.samples_per_pixel);
        let mut paths = Vec::new();
        match image.kind {
            LinearImageKind::Snapshot => {
                paths.extend(self.progress_dir.iter().map(|dir| dir.join(&snapshot_name)));
            }
            LinearImageKind::Final => {
                match (&self.target, image.frame_name) {
                    (Some(RawTarget::File(file)), _) => paths.push(file.clone()),
                    (Some(RawTarget::Folder(dir)), Some(name)) => {
                        paths.push(dir.join(format!("{name}.plraw")));
                    }
                    _ => {}
                }
                paths.extend(self.progress_dir.iter().map(|dir| dir.join(&snapshot_name)));
            }
        }
        paths
    }
}

/// Writes `bytes` to `path`, making the folder first.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

impl RawTapSink for RawFileSink {
    fn linear_image(&self, image: &LinearImage<'_>) {
        let destinations = self.destinations(image);
        if destinations.is_empty() {
            return;
        }
        let raw = RawImage {
            width: image.width,
            height: image.height,
            xyz: image.mean_xyz(),
            // The frame-wide count for every pixel, or each pixel's own (adaptive sampling).
            counts: image.pixel_counts(),
        };
        let bytes = match raw.to_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                self.fail(format!("cannot make the raw image: {e}"));
                return;
            }
        };
        for path in destinations {
            match write_file(&path, &bytes) {
                Ok(()) => {
                    self.written.fetch_add(1, Ordering::Relaxed);
                }
                Err(message) => self.fail(message),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glam::Vec3;

    fn image<'a>(
        kind: LinearImageKind,
        samples: u32,
        frame_name: Option<&'a str>,
        sums: &'a [Vec3],
    ) -> LinearImage<'a> {
        LinearImage {
            kind,
            width: 2,
            height: 1,
            samples_per_pixel: samples,
            sums,
            frame_name,
            counts: None,
        }
    }

    fn folder(name: &str) -> PathBuf {
        let folder = testing::temp_path(name);
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn a_snapshot_is_named_by_its_real_sample_count() {
        assert_eq!(snapshot_file_name(16), "spp_000016.plraw");
        assert_eq!(snapshot_file_name(33), "spp_000033.plraw");
        assert_eq!(snapshot_file_name(32_768), "spp_032768.plraw");
    }

    #[test]
    fn a_still_writes_its_file_and_a_progress_folder_gets_snapshots_and_the_final() {
        let dir = folder("raw-sink-still");
        let file = dir.join("out").join("still.plraw");
        let progress = dir.join("steps");
        let sink = RawFileSink::new(Some(RawTarget::File(file.clone())), Some(progress.clone()));
        let sums = [Vec3::new(8.0, 4.0, 2.0), Vec3::new(4.0, 2.0, 1.0)];
        sink.linear_image(&image(LinearImageKind::Snapshot, 16, None, &sums));
        sink.linear_image(&image(LinearImageKind::Final, 32, None, &sums));
        assert_eq!(sink.take_error(), None);
        assert_eq!(sink.files_written(), 3);

        let read = |path: &Path| {
            RawImage::from_bytes(&std::fs::read(path).expect("the file is there"))
                .expect("a PLRAW1 file")
        };
        let snapshot = read(&progress.join("spp_000016.plraw"));
        assert_eq!((snapshot.width, snapshot.height), (2, 1));
        assert_eq!(snapshot.counts, vec![16, 16]);
        assert_eq!(snapshot.xyz[0], [0.5, 0.25, 0.125]);
        let last = read(&file);
        assert_eq!(last.counts, vec![32, 32]);
        assert_eq!(last.xyz[1], [0.125, 0.0625, 0.03125]);
        assert_eq!(read(&progress.join("spp_000032.plraw")), last);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_pixel_counts_are_written_and_read_back() {
        let dir = folder("raw-sink-counts");
        let file = dir.join("adaptive.plraw");
        let progress = dir.join("steps");
        let sink = RawFileSink::new(Some(RawTarget::File(file.clone())), Some(progress.clone()));
        let means = [Vec3::new(0.5, 0.25, 0.125), Vec3::new(2.0, 1.0, 0.5)];
        let counts = [16_u32, 64];
        let adaptive = LinearImage {
            kind: LinearImageKind::Final,
            width: 2,
            height: 1,
            samples_per_pixel: 64,
            sums: &means,
            frame_name: None,
            counts: Some(&counts),
        };
        sink.linear_image(&adaptive);
        assert_eq!(sink.take_error(), None);
        let read = |path: &Path| {
            RawImage::from_bytes(&std::fs::read(path).expect("the file is there"))
                .expect("a PLRAW1 file")
        };
        let last = read(&file);
        assert_eq!(last.counts, vec![16, 64], "each pixel's own count");
        assert_eq!(last.xyz[0], [0.5, 0.25, 0.125], "the means as they are");
        assert_eq!(last.xyz[1], [2.0, 1.0, 0.5]);
        assert_eq!(
            read(&progress.join("spp_000064.plraw")),
            last,
            "a final image is named by the samples per pixel done"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_wide_count_still_writes_the_same_bytes_as_before() {
        let sums = [Vec3::new(8.0, 4.0, 2.0), Vec3::new(4.0, 2.0, 1.0)];
        let image = image(LinearImageKind::Final, 32, None, &sums);
        // The frame-wide form: the mean over one count, and the count repeated for every pixel.
        let before = RawImage {
            width: 2,
            height: 1,
            xyz: vec![[0.25, 0.125, 0.0625], [0.125, 0.0625, 0.03125]],
            counts: vec![32, 32],
        };
        assert_eq!(image.mean_xyz(), before.xyz);
        assert_eq!(image.pixel_counts(), before.counts);
        let dir = folder("raw-sink-frame-wide");
        let file = dir.join("plain.plraw");
        let sink = RawFileSink::new(Some(RawTarget::File(file.clone())), None);
        sink.linear_image(&image);
        assert_eq!(
            std::fs::read(&file).expect("the file is there"),
            before.to_bytes().expect("the image encodes")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_video_frame_is_written_under_its_png_name_and_a_still_target_needs_none() {
        let dir = folder("raw-sink-video");
        let sink = RawFileSink::new(Some(RawTarget::Folder(dir.clone())), None);
        let sums = [Vec3::ONE, Vec3::ONE];
        sink.linear_image(&image(LinearImageKind::Final, 4, Some("frame_007"), &sums));
        // A snapshot with no progress folder, and a frame without a name, write nothing.
        sink.linear_image(&image(
            LinearImageKind::Snapshot,
            16,
            Some("frame_007"),
            &sums,
        ));
        sink.linear_image(&image(LinearImageKind::Final, 4, None, &sums));
        assert_eq!(sink.files_written(), 1);
        assert!(dir.join("frame_007.plraw").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_is_kept_for_the_command_to_report() {
        let dir = folder("raw-sink-fails");
        std::fs::create_dir_all(&dir).expect("the folder can be made");
        // A file where the folder should be: the write cannot succeed.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("the blocker can be written");
        let sink = RawFileSink::new(Some(RawTarget::File(blocker.join("out.plraw"))), None);
        sink.linear_image(&image(
            LinearImageKind::Final,
            4,
            None,
            &[Vec3::ONE, Vec3::ONE],
        ));
        let error = sink.take_error().expect("the failure is kept");
        assert!(error.contains("cannot"), "{error}");
        assert_eq!(sink.files_written(), 0);
        assert_eq!(sink.take_error(), None, "taking it clears it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
