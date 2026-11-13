//! Wavefront OBJ: vertices (`v`) and faces (`f`); everything else is ignored.

use super::{NotedMesh, ReadProgress, face::triangulate_face, too_many_triangles_note};
use glam::DVec3;
use pleochroma_cut_core::rough_plan::MAX_MESH_TRIANGLES;

/// The vertices (`v x y z`, in the file's unit) and triangles (0-based indices into the
/// vertices) of the OBJ text `text`.
///
/// A face (`f`) may name its vertices as `v`, `v/vt`, `v//vn` or `v/vt/vn`, with 1-based
/// or negative (relative to the vertices read so far) indices; a convex polygon is fanned
/// from its first vertex and a non-convex one is ear-clipped in its plane. Normals,
/// textures and everything else are ignored. A file with vertices but no faces gives no
/// triangles.
///
/// # Errors
///
/// Returns the message for a vertex line without three numbers, a face with fewer than
/// three vertices, an index that is not a number or points outside the vertices, or a file
/// without vertices.
#[cfg(test)]
pub fn parse_obj_mesh(text: &str) -> Result<(Vec<DVec3>, Vec<[u32; 3]>), String> {
    parse_obj_noted(text).map(|(points, triangles, _)| (points, triangles))
}

/// The points and triangles of an OBJ file, with a note when the triangles are not used.
///
/// A non-convex polygon face that cannot be split, or more triangles than the planner takes,
/// leaves the triangles empty: the rough is then the convex hull of the points, because a mesh
/// with that face left out would be open or wrong.
///
/// Past [`MAX_MESH_TRIANGLES`] the faces already read are dropped and the rest of the file
/// is only counted, so a scan of millions of triangles costs no memory beyond its vertices.
///
/// # Errors
///
/// As [`parse_obj_mesh`].
#[cfg(test)]
pub fn parse_obj_noted(text: &str) -> Result<NotedMesh, String> {
    parse_obj_progress(text, &ReadProgress::default())
}

/// How many triangles the faces of the OBJ text `text` give (a face of `n` corners gives
/// `n - 2`), counted in a quick pass without parsing a number: the total a read's progress is
/// measured against.
#[must_use]
pub fn count_triangles(text: &str) -> usize {
    text.lines()
        .filter(|line| line.starts_with("f ") || line.starts_with("f\t"))
        .map(|line| line.split_whitespace().count().saturating_sub(3))
        .sum()
}

/// Parses the OBJ text as [`super::parse_mesh`] does, reporting to `progress`.
///
/// The triangles of every face are added to `progress` as the face is read. Faces past the
/// planner's limit are counted too.
///
/// # Errors
///
/// As [`super::parse_mesh`].
pub fn parse_obj_progress(text: &str, progress: &ReadProgress) -> Result<NotedMesh, String> {
    let mut points = Vec::new();
    // Faces as signed indices, negative ones already made absolute; positive ones are
    // checked against the final vertex count (a face may name a later vertex).
    let mut faces: Vec<(usize, Vec<i64>)> = Vec::new();
    let mut triangle_count = 0_usize;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let mut coordinate = || {
                    words
                        .next()
                        .and_then(|word| word.parse::<f64>().ok())
                        .ok_or_else(|| format!("line {line_no} is not a vertex with three numbers"))
                };
                points.push(DVec3::new(coordinate()?, coordinate()?, coordinate()?));
            }
            Some("f") if triangle_count > MAX_MESH_TRIANGLES => {
                let more = words.count().saturating_sub(2);
                triangle_count += more;
                progress.add_triangles(more);
            }
            Some("f") => {
                let count = i64::try_from(points.len()).unwrap_or(i64::MAX);
                let mut corners = Vec::new();
                for word in words {
                    let raw = word.split('/').next().unwrap_or_default();
                    let value: i64 = raw
                        .parse()
                        .map_err(|_| format!("line {line_no} has a face index `{word}`"))?;
                    // 1-based; `-1` is the latest vertex.
                    let absolute = match value {
                        0 => return Err(format!("line {line_no} has the face index 0")),
                        v if v < 0 => count + v + 1,
                        v => v,
                    };
                    corners.push(absolute);
                }
                if corners.len() < 3 {
                    return Err(format!(
                        "line {line_no} is a face with fewer than three vertices"
                    ));
                }
                triangle_count += corners.len() - 2;
                progress.add_triangles(corners.len() - 2);
                if triangle_count > MAX_MESH_TRIANGLES {
                    faces = Vec::new();
                } else {
                    faces.push((line_no, corners));
                }
            }
            _ => {}
        }
    }
    if points.is_empty() {
        return Err("the file has no vertices (`v x y z` lines)".to_string());
    }
    if triangle_count > MAX_MESH_TRIANGLES {
        return Ok((
            points,
            Vec::new(),
            Some(too_many_triangles_note(triangle_count)),
        ));
    }
    let total = i64::try_from(points.len()).unwrap_or(i64::MAX);
    let mut triangles = Vec::new();
    let mut note = None;
    for (line_no, corners) in faces {
        let mut resolved = Vec::with_capacity(corners.len());
        for corner in corners {
            if !(1..=total).contains(&corner) {
                return Err(format!(
                    "line {line_no} names vertex {corner}, but the file has {total} vertices"
                ));
            }
            // In range of `1..=total`, so it fits a `u32` for any file the mesh limit lets
            // through; a larger vertex count is refused by the cast.
            resolved.push(
                u32::try_from(corner - 1)
                    .map_err(|_| "the file has too many vertices".to_string())?,
            );
        }
        match triangulate_face(&points, &resolved) {
            Some(split) => triangles.extend(split),
            None => {
                note.get_or_insert_with(|| {
                    format!(
                        "Line {line_no} is a polygon face that is not convex and could not be \
                         split into triangles, so the mesh is not used and its convex hull is \
                         the rough."
                    )
                });
            }
        }
    }
    if note.is_some() {
        triangles.clear();
    }
    Ok((points, triangles, note))
}
