//! Inclusions of a block, cylinder or pebble rough.
//!
//! A rough imported from a mesh file keeps its inclusions inside its registered mesh (see
//! [`hull`]). A block, a cylinder or a pebble has no mesh and no file, so its inclusions live
//! on the model: an [`InclusionSet`], the grown shells in the frame of the uncut base (the
//! frame [`RoughBase::vertices`] and the planes already use, with the base's corner at the
//! origin). The base stays analytic, so its sizes and the block's edge and corner cuts stay
//! editable.
//!
//! # The mesh of a rough with inclusions
//!
//! With a non-empty set, [`RoughModel::mesh`] returns the convex outer mesh of the base with the
//! shells inside it, and the planner then treats the rough like any mesh rough (the shaped
//! path, the mesh's blockers and rows). The outer mesh has the very corners of the plane
//! polytope, so the volume, the weight and the yield do not jump when the first inclusion is
//! added. The mesh is derived, never stored in the set: a set is shared by every clone of a
//! model (undo entries, a plan job), and clones differ in their base after a size edit or Fit
//! to weight. It is cached in a small registry keyed by the base and the set, which is the
//! same registry-by-content idea as the hull registry, and the cuts are not part of the key.
//!
//! # Edits that push a shell out
//!
//! A size edit, an edge or corner cut that no longer exists, or Fit to weight can leave a shell
//! outside the material or through the surface. [`RoughModel::mesh_checked`] says so with
//! [`ShapeError::Inclusion`], and [`RoughModel::solid`] (so the measurement, the planner's
//! context and the app's model check) fails with it: nothing is dropped or clamped. A cut
//! that slices a shell is legal, as for a mesh rough.

use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use super::{
    HullError, MAX_MESH_TRIANGLES, MeshError, RoughBase, RoughMesh, RoughModel, ShapeError,
    hull::{self, InclusionInfo, MAX_INCLUSIONS, MeshParts, convex_outer_mesh},
};

/// How many derived meshes the registry keeps. A build is deterministic, so a mesh that
/// falls out costs only the time to build it again.
const CACHE_CAP: usize = 16;

/// Why an inclusion of a block, cylinder or pebble does not fit its rough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InclusionFault {
    /// The shell lies outside the rough's material.
    Outside,
    /// The shell crosses or reaches the rough's surface.
    ReachesSurface,
    /// The shell crosses another shell, or lies inside it.
    Overlap,
    /// The shells together have more triangles than a mesh may have.
    TooComplex,
    /// The rough's own outline cannot be turned into a mesh.
    Unusable,
    /// The base is an imported mesh, which keeps its inclusions in its own mesh.
    NotForBase,
}

impl fmt::Display for InclusionFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Outside => "it lies outside the rough's material",
            Self::ReachesSurface => "it reaches the surface of the rough",
            Self::Overlap => "it crosses another inclusion",
            Self::TooComplex => "the inclusions have too many triangles together",
            Self::Unusable => "the rough's outline cannot hold inclusions",
            Self::NotForBase => "an imported mesh keeps its inclusions in its own mesh",
        })
    }
}

/// The shells of one set and the hash of their content.
struct SetInner {
    /// Each shell as a closed solid wound outward, in the base frame, already grown by its
    /// margin.
    shells: Vec<RoughMesh>,
    /// The content id (never 0).
    id: u64,
}

/// The inclusions of a block, cylinder or pebble rough: grown closed shells in the frame of
/// the uncut base.
///
/// Cloning is cheap (the shells are shared) and equality compares a content id, so a model
/// holding a set compares and clones like one without. The empty set is [`EMPTY`](Self::EMPTY).
#[derive(Clone, Default)]
pub struct InclusionSet {
    inner: Option<Arc<SetInner>>,
}

impl fmt::Debug for InclusionSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InclusionSet")
            .field("len", &self.len())
            .field("id", &self.id())
            .finish()
    }
}

impl PartialEq for InclusionSet {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for InclusionSet {}

/// FNV-1a over `words`, folded into `hash`.
fn fnv(mut hash: u64, words: impl IntoIterator<Item = u64>) -> u64 {
    for value in words {
        for byte in value.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// A count as a hash word.
fn word(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// The content id of `shells`: their vertices and triangles, bit for bit.
fn content_id(shells: &[RoughMesh]) -> u64 {
    let mut hash = fnv(0xcbf2_9ce4_8422_2325, [word(shells.len())]);
    for shell in shells {
        hash = fnv(
            hash,
            [word(shell.vertices().len()), word(shell.triangles().len())],
        );
        // `+ 0.0` turns a negative zero into a zero, which compares equal but hashes apart.
        hash = fnv(
            hash,
            shell
                .vertices()
                .iter()
                .flat_map(|v| v.to_array().map(|c| (c + 0.0).to_bits())),
        );
        hash = fnv(
            hash,
            shell.triangles().iter().flatten().map(|&i| u64::from(i)),
        );
    }
    hash.max(1)
}

impl InclusionSet {
    /// The set without inclusions: every rough that has none.
    pub const EMPTY: Self = Self { inner: None };

    /// The set of `shells` (empty when there are none).
    fn from_shells(shells: Vec<RoughMesh>) -> Self {
        if shells.is_empty() {
            return Self::EMPTY;
        }
        let id = content_id(&shells);
        Self {
            inner: Some(Arc::new(SetInner { shells, id })),
        }
    }

    /// The set of `shells` (closed solids wound outward, in the base frame, already grown by
    /// whatever margin they are meant to have).
    ///
    /// # Errors
    ///
    /// [`HullError::TooManyInclusions`] for more than [`MAX_INCLUSIONS`] shells.
    pub fn from_meshes(shells: Vec<RoughMesh>) -> Result<Self, HullError> {
        if shells.len() > MAX_INCLUSIONS {
            return Err(HullError::TooManyInclusions);
        }
        Ok(Self::from_shells(shells))
    }

    /// The set a saved plan stores as `parts` (points in mm and triangles of each shell, the
    /// shells grown already): the inverse of [`to_parts`](Self::to_parts).
    ///
    /// Whether the shells fit a base is checked by [`RoughModel::mesh_checked`].
    ///
    /// # Errors
    ///
    /// [`HullError::TooManyInclusions`] for more than [`MAX_INCLUSIONS`] shells and
    /// [`HullError::Inclusion`] for a shell that is not a usable closed mesh.
    pub fn from_parts(parts: &[MeshParts]) -> Result<Self, HullError> {
        if parts.len() > MAX_INCLUSIONS {
            return Err(HullError::TooManyInclusions);
        }
        let shells = parts
            .iter()
            .map(|(points, tris)| RoughMesh::new(points, tris).map_err(HullError::Inclusion))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_shells(shells))
    }

    /// The shells as points in mm and triangles, in the order they were added, which is what
    /// a saved plan stores: [`from_parts`](Self::from_parts) reads them back to a set with the
    /// same [`id`](Self::id).
    #[must_use]
    pub fn to_parts(&self) -> Vec<MeshParts> {
        self.shells()
            .iter()
            .map(|shell| (shell.vertices().to_vec(), shell.triangles().to_vec()))
            .collect()
    }

    /// Whether there are no inclusions. Usable in a `const fn`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.inner.is_none()
    }

    /// How many inclusions there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shells().len()
    }

    /// The content id: 0 for the empty set, else a hash of every shell's vertices and
    /// triangles. Two sets with one id are one set.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.inner.as_ref().map_or(0, |inner| inner.id)
    }

    /// The shells, grown, in the order they were added.
    #[must_use]
    pub fn shells(&self) -> &[RoughMesh] {
        match &self.inner {
            Some(inner) => inner.shells.as_slice(),
            None => &[],
        }
    }

    /// Each inclusion as the planner window lists it (volume with its margin, centre and
    /// extents of its bounding box, in the base frame).
    #[must_use]
    pub fn infos(&self) -> Vec<InclusionInfo> {
        self.shells()
            .iter()
            .map(|body| {
                let (lo, hi) = body.bounds();
                InclusionInfo {
                    volume_mm3: body.volume(),
                    centre_mm: (lo + hi) * 0.5,
                    extents_mm: hi - lo,
                }
            })
            .collect()
    }

    /// The set with every length multiplied by `factor`, as Fit to weight scales the rough.
    ///
    /// An unusable factor (not finite and positive) leaves the set as it is; the base it goes
    /// with is invalid then anyway.
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        if self.is_empty() {
            return Self::EMPTY;
        }
        let scaled: Option<Vec<RoughMesh>> = self
            .shells()
            .iter()
            .map(|shell| shell.scaled(factor))
            .collect();
        scaled.map_or_else(|| self.clone(), Self::from_shells)
    }

    /// The set with `extra` shells (already grown) after its own.
    ///
    /// # Errors
    ///
    /// [`HullError::TooManyInclusions`] when the set would pass [`MAX_INCLUSIONS`].
    pub fn appended(&self, extra: Vec<RoughMesh>) -> Result<Self, HullError> {
        if self.len() + extra.len() > MAX_INCLUSIONS {
            return Err(HullError::TooManyInclusions);
        }
        let mut shells = self.shells().to_vec();
        shells.extend(extra);
        Ok(Self::from_shells(shells))
    }

    /// The set without inclusion `index` (0-based, in the order added); `None` when there is
    /// no such inclusion.
    #[must_use]
    pub fn without(&self, index: usize) -> Option<Self> {
        if index >= self.len() {
            return None;
        }
        let rest: Vec<RoughMesh> = self
            .shells()
            .iter()
            .enumerate()
            .filter(|&(k, _)| k != index)
            .map(|(_, shell)| shell.clone())
            .collect();
        Some(Self::from_shells(rest))
    }
}

/// One derived mesh of the registry.
struct Cached {
    base: RoughBase,
    set_id: u64,
    mesh: Arc<RoughMesh>,
}

/// The derived meshes built most recently, oldest first.
static CACHE: Mutex<VecDeque<Cached>> = Mutex::new(VecDeque::new());

/// The registry, whatever happened to a thread that held it before.
fn cache() -> MutexGuard<'static, VecDeque<Cached>> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The registered mesh of `base` with the set `set_id`, made the most recent.
fn cached(base: &RoughBase, set_id: u64) -> Option<Arc<RoughMesh>> {
    let mut entries = cache();
    let at = entries
        .iter()
        .position(|entry| entry.set_id == set_id && entry.base == *base)?;
    let hit = entries.remove(at)?;
    let mesh = Arc::clone(&hit.mesh);
    entries.push_back(hit);
    drop(entries);
    Some(mesh)
}

/// Registers `mesh` for `base` and the set `set_id`; when another thread registered one first,
/// that one, so both share it.
fn store(base: &RoughBase, set_id: u64, mesh: Arc<RoughMesh>) -> Arc<RoughMesh> {
    let mut entries = cache();
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.set_id == set_id && entry.base == *base)
    {
        return Arc::clone(&entry.mesh);
    }
    entries.push_back(Cached {
        base: *base,
        set_id,
        mesh: Arc::clone(&mesh),
    });
    while entries.len() > CACHE_CAP {
        entries.pop_front();
    }
    drop(entries);
    mesh
}

/// How many derived meshes the registry holds (at most [`CACHE_CAP`]).
#[cfg(test)]
pub(super) fn cached_meshes() -> usize {
    cache().len()
}

/// The convex outer mesh of an analytic `base`: the triangles of the hull of its fine
/// polytope's corners, which are the corners of the planes the planner uses.
fn outer_mesh(base: &RoughBase) -> Result<RoughMesh, ShapeError> {
    let corners = base.vertices(false)?;
    convex_outer_mesh(&corners).map_err(|_| ShapeError::Inclusion {
        index: 0,
        reason: InclusionFault::Unusable,
    })
}

/// The first shell of `shells` that fails once it is added to `outer` after its
/// predecessors, for an overlap that the mesh does not number.
fn first_overlap(outer: &RoughMesh, shells: &[RoughMesh]) -> usize {
    (1..shells.len())
        .find(|&k| RoughMesh::with_inclusions(outer, &shells[..=k]).is_err())
        .unwrap_or(0)
}

/// The mesh of `outer` with `shells` inside it, or what is wrong with them.
fn build(outer: &RoughMesh, shells: &[RoughMesh]) -> Result<RoughMesh, ShapeError> {
    RoughMesh::with_inclusions(outer, shells).map_err(|error| {
        let (index, reason) = match error {
            MeshError::InclusionOutside(index) => (index, InclusionFault::Outside),
            MeshError::InclusionReachesSurface(index) => (index, InclusionFault::ReachesSurface),
            MeshError::InclusionsOverlap => (first_overlap(outer, shells), InclusionFault::Overlap),
            MeshError::TooManyTriangles(_) => (0, InclusionFault::TooComplex),
            _ => (0, InclusionFault::Unusable),
        };
        ShapeError::Inclusion { index, reason }
    })
}

/// The mesh of `base` with `set` inside, from the registry or built. `None` for the empty
/// set.
fn derived_mesh(
    base: &RoughBase,
    set: &InclusionSet,
) -> Result<Option<Arc<RoughMesh>>, ShapeError> {
    if set.is_empty() {
        return Ok(None);
    }
    if let Some(mesh) = cached(base, set.id()) {
        return Ok(Some(mesh));
    }
    let outer = outer_mesh(base)?;
    let mesh = Arc::new(build(&outer, set.shells())?);
    Ok(Some(store(base, set.id(), mesh)))
}

/// The error of [`RoughModel::with_inclusions_added`] for a rough whose inclusions are
/// already wrong.
const fn hull_error_of(error: &ShapeError) -> HullError {
    match *error {
        ShapeError::Inclusion { index, reason } => HullError::Inclusion(match reason {
            InclusionFault::Outside => MeshError::InclusionOutside(index),
            InclusionFault::ReachesSurface => MeshError::InclusionReachesSurface(index),
            InclusionFault::Overlap => MeshError::InclusionsOverlap,
            InclusionFault::TooComplex => MeshError::TooManyTriangles(MAX_MESH_TRIANGLES + 1),
            InclusionFault::Unusable | InclusionFault::NotForBase => MeshError::Degenerate,
        }),
        _ => HullError::OuterUnusable(MeshError::Degenerate),
    }
}

impl RoughModel {
    /// [`mesh`](Self::mesh) that says why a rough with inclusions has none.
    ///
    /// `Ok(None)` for every convex rough without inclusions, the registered mesh of an imported
    /// rough, and for a block, cylinder or pebble with inclusions the convex outer mesh of its
    /// base with the shells inside it (shared through a registry keyed by base and set, never
    /// stored in the set).
    ///
    /// # Errors
    ///
    /// [`ShapeError`] when the base is invalid, or [`ShapeError::Inclusion`] when a shell lies
    /// outside the material, reaches the surface, crosses another shell, or the model is an
    /// imported mesh that also carries an [`InclusionSet`].
    pub fn mesh_checked(&self) -> Result<Option<Arc<RoughMesh>>, ShapeError> {
        if let RoughBase::Hull { id, .. } = self.base {
            return if self.inclusions.is_empty() {
                Ok(hull::mesh(id))
            } else {
                Err(ShapeError::Inclusion {
                    index: 0,
                    reason: InclusionFault::NotForBase,
                })
            };
        }
        derived_mesh(&self.base, &self.inclusions)
    }

    /// How many inclusions the rough has, whichever way it keeps them.
    #[must_use]
    pub fn inclusion_count(&self) -> usize {
        match self.base {
            RoughBase::Hull { id, .. } => hull::mesh(id).map_or(0, |mesh| mesh.inclusion_count()),
            _ => self.inclusions.len(),
        }
    }

    /// The inclusions as the planner window lists them, in the order added, whichever way the
    /// rough keeps them.
    #[must_use]
    pub fn inclusion_infos(&self) -> Vec<InclusionInfo> {
        match self.base {
            RoughBase::Hull { id, .. } => hull::inclusion_list(id),
            _ => self.inclusions.infos(),
        }
    }

    /// The model with `bodies` added as inclusions, each grown by `margin_mm` first (see
    /// [`RoughMesh::grown`]).
    ///
    /// Each body is a closed solid wound outward in the frame of the uncut base (the corner of
    /// the base at the origin; for an imported mesh, the frame of [`hull::add_inclusion_meshes`]).
    /// It must lie wholly in the material and must not cross the surface or another inclusion.
    /// An imported mesh gets a new base (as [`hull::add_inclusion_meshes`] gives); any other
    /// base stays and the model's [`InclusionSet`] grows. The model itself is unchanged.
    ///
    /// # Errors
    ///
    /// As [`hull::add_inclusion_meshes`]: [`HullError::Inclusion`] for a body that does not fit
    /// or when the inclusions it already has do not fit, [`HullError::InclusionTooClose`] for
    /// one that fits only without its margin, [`HullError::TooManyInclusions`], and
    /// [`HullError::OuterUnusable`] when the outline cannot be meshed.
    pub fn with_inclusions_added(
        &self,
        bodies: &[RoughMesh],
        margin_mm: f64,
    ) -> Result<Self, HullError> {
        let mut model = self.clone();
        if let RoughBase::Hull { .. } = self.base {
            model.base = hull::add_inclusion_meshes(&self.base, bodies, margin_mm)?;
            return Ok(model);
        }
        if bodies.is_empty() {
            return Ok(model);
        }
        if self.inclusions.len() + bodies.len() > MAX_INCLUSIONS {
            return Err(HullError::TooManyInclusions);
        }
        // The inclusions the rough has must fit it before another is added.
        self.check_inclusions()
            .map_err(|error| hull_error_of(&error))?;
        // The plain outer mesh, without the existing shells: a body is checked against the
        // surface alone, and against every shell (old and new) in the combined build below, so
        // a body that crosses or wraps an existing inclusion is said to overlap it, not to reach
        // the surface, and is refused here instead of making the model an error later.
        let outer = outer_mesh(&self.base).map_err(|error| hull_error_of(&error))?;
        let mut grown = Vec::with_capacity(bodies.len());
        for (k, body) in bodies.iter().enumerate() {
            // As given first, so a body that reaches the surface is said so, not blamed on the
            // margin.
            outer
                .check_inclusion(body, k)
                .map_err(HullError::Inclusion)?;
            grown.push(body.grown(margin_mm).map_err(HullError::Inclusion)?);
        }
        let mut all = self.inclusions.shells().to_vec();
        all.extend(grown);
        let combined = RoughMesh::with_inclusions(&outer, &all).map_err(|error| match error {
            MeshError::InclusionOutside(_)
            | MeshError::InclusionReachesSurface(_)
            | MeshError::InclusionsOverlap
                if margin_mm > 0.0 =>
            {
                HullError::InclusionTooClose
            }
            other => HullError::Inclusion(other),
        })?;
        model.inclusions = InclusionSet::from_meshes(combined.inclusions().to_vec())?;
        // The combined mesh is the one the model's next `mesh()` would build: register it.
        store(&model.base, model.inclusions.id(), Arc::new(combined));
        Ok(model)
    }

    /// The model without inclusion `index` (0-based, in the order added), whichever way the
    /// rough keeps them. The model itself is unchanged.
    ///
    /// # Errors
    ///
    /// [`HullError::NoInclusion`] when the rough has no inclusion `index`, and for an imported
    /// mesh the errors of [`hull::remove_inclusion`].
    pub fn with_inclusion_removed(&self, index: usize) -> Result<Self, HullError> {
        let mut model = self.clone();
        if let RoughBase::Hull { .. } = self.base {
            model.base = hull::remove_inclusion(&self.base, index)?;
        } else {
            model.inclusions = self
                .inclusions
                .without(index)
                .ok_or(HullError::NoInclusion)?;
        }
        Ok(model)
    }

    /// The model with `inclusions` as its set (the shells are checked against the base when
    /// the mesh is asked for, see [`mesh_checked`](Self::mesh_checked)).
    #[must_use]
    pub fn with_inclusion_set(mut self, inclusions: InclusionSet) -> Self {
        self.inclusions = inclusions;
        self
    }

    /// Checks that the inclusions fit the rough as it is now.
    ///
    /// # Errors
    ///
    /// As [`mesh_checked`](Self::mesh_checked).
    pub fn check_inclusions(&self) -> Result<(), ShapeError> {
        self.mesh_checked().map(|_| ())
    }
}
