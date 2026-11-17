//! A finished job's picture, for `FetchResult`: found in the job store, then cut into the
//! chunks the protocol sends.

use crate::serve::ServeCore;
use pleochroma_net::admin::{
    protocol::{AdminReply, RESULT_CHUNK_LEN},
    status::JobId,
};

/// What `FetchResult` answers when the job store has nothing to give.
const NO_JOBS: &str =
    "This coordinator keeps no jobs. Jobs need \"Render on this computer\" to be on.";

/// The picture (PNG bytes) of the finished job `job`, whoever owns it.
///
/// # Errors
///
/// A plain sentence when this coordinator keeps no jobs (also every build without render
/// support), the job is unknown, it is not done, or its file cannot be read.
pub fn load(host: &ServeCore, job: JobId) -> Result<Vec<u8>, String> {
    #[cfg(feature = "worker")]
    {
        use crate::jobs::JobError;

        let admin = host.job_admin.as_ref().ok_or_else(|| NO_JOBS.to_owned())?;
        // The job's owner is found through the administrator's view of the store.
        let (owner, _info) = admin
            .list()
            .into_iter()
            .find(|(_, info)| info.job_id == job.0)
            .ok_or_else(|| "That job no longer exists.".to_owned())?;
        admin
            .service()
            .store()
            .result(&owner, &job.0)
            .map(|(_info, bytes)| bytes)
            .map_err(|error| match error {
                JobError::Unknown => "That job no longer exists.".to_owned(),
                JobError::State(text) | JobError::Invalid(text) | JobError::Limit(text) => {
                    capitalize(&text)
                }
                JobError::Io(text) => {
                    tracing::warn!("admin fetch: {text}");
                    "The picture could not be read. The log has the details.".to_owned()
                }
            })
    }
    #[cfg(not(feature = "worker"))]
    {
        let _ = (host, job);
        Err(NO_JOBS.to_owned())
    }
}

/// `text` with its first character in upper case.
#[cfg(feature = "worker")]
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The replies that carry `png`: pieces of at most [`RESULT_CHUNK_LEN`] bytes in order, the
/// last one flagged. An empty picture is one empty, last piece.
pub fn chunk_replies(job: JobId, png: &[u8]) -> impl Iterator<Item = AdminReply> + '_ {
    let total_len = png.len() as u64;
    let count = png.len().div_ceil(RESULT_CHUNK_LEN).max(1);
    (0..count).map(move |index| {
        let start = (index * RESULT_CHUNK_LEN).min(png.len());
        let end = (start + RESULT_CHUNK_LEN).min(png.len());
        AdminReply::ResultChunk {
            job,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            total_len,
            data: png[start..end].to_vec(),
            last: index + 1 == count,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(len: usize) -> Vec<(u32, usize, bool)> {
        let png = vec![7u8; len];
        chunk_replies(JobId([3; 16]), &png)
            .map(|reply| match reply {
                AdminReply::ResultChunk {
                    index,
                    data,
                    last,
                    total_len,
                    job,
                } => {
                    assert_eq!(total_len, len as u64);
                    assert_eq!(job, JobId([3; 16]));
                    (index, data.len(), last)
                }
                other => panic!("not a chunk: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_picture_is_cut_into_ordered_pieces_with_the_last_one_flagged() {
        assert_eq!(pieces(0), vec![(0, 0, true)]);
        assert_eq!(pieces(1), vec![(0, 1, true)]);
        assert_eq!(pieces(RESULT_CHUNK_LEN), vec![(0, RESULT_CHUNK_LEN, true)]);
        assert_eq!(
            pieces(RESULT_CHUNK_LEN + 5),
            vec![(0, RESULT_CHUNK_LEN, false), (1, 5, true)]
        );
        let three = pieces(RESULT_CHUNK_LEN * 2 + 1);
        assert_eq!(three.len(), 3);
        assert_eq!(three.iter().filter(|(_, _, last)| *last).count(), 1);
        assert_eq!(
            three.iter().map(|(_, len, _)| len).sum::<usize>(),
            RESULT_CHUNK_LEN * 2 + 1
        );
    }
}
