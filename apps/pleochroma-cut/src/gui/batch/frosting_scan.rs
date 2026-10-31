//! The frosting half of the catalogue staleness scans.
//!
//! A cached preview or tilt-curve set is stored under a fingerprint that names the design's
//! frosting (`bridge::preview_render::FingerprintMaterial::frosting`), so a frosted design
//! never reuses an unfrosted picture. The vault's scans
//! (`Database::entry_ids_missing_previews` / `entry_ids_missing_tilt_curves`) hand their
//! fingerprint function only the persisted material state, not the entry, and the frosting
//! lives in a side table (`Database::diagram_frosting`). So the scan is run once for the
//! polished fingerprint and once for each distinct frosting a stored design has, and each
//! design is judged by the run that matches its own frosting.

use crate::bridge::preview_render::{AxisKey, EffectsKey, frosting_hash, zones_hash};
use pleochroma::optics::raytracer::surface::FacetingMachine;
use pleochroma_vault::{db::sqlite::Database, model::preview::PreviewMaterialState};
use std::collections::{BTreeMap, BTreeSet};

/// The [`frosting_hash`] of `entry_id`'s stored frosting; `0` for a polished design (and for
/// a read that fails, which is the polished fingerprint, the one every earlier row has).
#[must_use]
pub fn stored_frosting_hash(db: &Database, entry_id: i64) -> u64 {
    frosting_hash(&db.diagram_frosting(entry_id).unwrap_or_default())
}

/// The [`EffectsKey`] of the surface finish, abraded edge, inclusions and play-of-color
/// `entry_id` states in the library ([`EffectsKey::of_stored`] and
/// [`EffectsKey::with_stored_inclusions`], with the user's faceting machine); [`EffectsKey::NONE`] for a
/// design that states none and for a read that fails (the fingerprint every earlier row has).
#[must_use]
pub fn stored_effects_key(db: &Database, entry_id: i64) -> EffectsKey {
    let surfaces = db
        .diagram_surface_finish(entry_id)
        .map_or(EffectsKey::NONE, |finish| {
            EffectsKey::of_stored(&finish, &crate::settings::model::current_faceting_machine())
        });
    surfaces.with_stored_inclusions(
        db.diagram_inclusion_effects(entry_id)
            .unwrap_or_default()
            .as_deref(),
    )
}

/// The [`zones_hash`] of the stored zones (`material_zoning.zoned_json`, the raw stored
/// text) of custom material `name`; `0` for no name, no zones or a failed read (the
/// unpatterned fingerprint, the one every earlier row has).
#[must_use]
pub fn stored_zones_hash(db: &Database, name: Option<&str>) -> u64 {
    name.and_then(|name| db.load_material_zoning(name).ok().flatten())
        .map_or(0, |row| zones_hash(&row.zoned_json))
}

/// [`stored_zones_hash`] of every material that has zones, read once, keyed by lower-cased
/// name: what a staleness scan consults per row instead of one query each.
#[must_use]
pub fn zones_hash_table(db: &Database) -> BTreeMap<String, u64> {
    db.all_material_zonings()
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            (
                row.material_name.to_lowercase(),
                zones_hash(&row.zoned_json),
            )
        })
        .collect()
}

/// A [`zones_hash_table`] entry for `name`; `0` for no name or no zones.
#[must_use]
pub fn zones_hash_in(table: &BTreeMap<String, u64>, name: Option<&str>) -> u64 {
    name.and_then(|name| table.get(&name.to_lowercase()))
        .copied()
        .unwrap_or(0)
}

/// The default crystal axis of every custom material that stores one, keyed by lower-cased
/// name: what a staleness scan and a save consult, as they consult [`zones_hash_table`]. A
/// material without a stored axis (both angles) is not listed, which is the fingerprint every
/// earlier row has.
#[must_use]
pub fn axis_key_table(db: &Database) -> BTreeMap<String, AxisKey> {
    db.get_custom_materials()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let key = AxisKey::from_stored(
                row.c_axis_tilt_deg,
                row.c_axis_azimuth_deg,
                row.c_axis_second_azimuth_deg,
            )?;
            Some((row.name.to_lowercase(), key))
        })
        .collect()
}

/// An [`axis_key_table`] entry for `name`; `None` for no name or no stored axis.
#[must_use]
pub fn axis_key_in(table: &BTreeMap<String, AxisKey>, name: Option<&str>) -> Option<AxisKey> {
    name.and_then(|name| table.get(&name.to_lowercase()))
        .copied()
}

/// The stored default axis of custom material `name`, read for one save.
#[must_use]
pub fn stored_axis_key(db: &Database, name: Option<&str>) -> Option<AxisKey> {
    let name = name?;
    axis_key_in(&axis_key_table(db), Some(name))
}

/// The crystal axis design `entry_id`'s own file sets (`Database::preview_axis`); `None` when
/// its override is off, for a design that never set one, and for a read that fails (the
/// fingerprint every earlier row has).
#[must_use]
pub fn stored_design_axis_key(db: &Database, entry_id: i64) -> Option<AxisKey> {
    db.preview_axis(entry_id)
        .unwrap_or_default()
        .map(AxisKey::from_preview)
}

/// The crystal axis a design is drawn with, as its fingerprint names it: the design's own
/// axis when its file turns the override on, else the stored default of the custom material
/// `material_name` ([`stored_axis_key`]); `None` for the material's built-in axis. The
/// writers (previews, tilt curves) use this, and the scans rebuild the same text from the
/// same two rows (`outdated_ids` for the design's, [`axis_key_table`] for the material's).
#[must_use]
pub fn effective_axis_key(
    db: &Database,
    entry_id: i64,
    material_name: Option<&str>,
) -> Option<AxisKey> {
    stored_design_axis_key(db, entry_id).or_else(|| stored_axis_key(db, material_name))
}

/// The row state a scan cannot read per row from the persisted material state: a design's
/// frosting hash, its own crystal axis and the key of its stored surface finish.
type DesignState = (u64, Option<AxisKey>, EffectsKey);

/// The grouping key of a design state: the frosting hash, the axis bits and the effects key.
type StateKey = (u64, Option<(u32, u32, Option<u32>)>, EffectsKey);

/// The entry ids whose stored picture is outdated, with frosting and the design's own
/// crystal axis taken into account.
///
/// `query` runs one of the vault's scans with the fingerprint function it is given;
/// `fingerprint` builds a row's current fingerprint from its persisted material state, the
/// [`frosting_hash`] of its design (`0` for none) and the crystal axis the design's own file
/// sets (`None` for none; the caller falls back to the material's stored default). A design
/// with neither is outdated when the plain run lists it; any other design when the run for
/// its own frosting and axis does. A scan that fails lists nothing, as the scans did before.
pub fn outdated_ids(
    db: &Database,
    fingerprint: impl Fn(&PreviewMaterialState, u64, Option<AxisKey>) -> String,
    query: impl Fn(&Database, &dyn Fn(&PreviewMaterialState) -> String) -> anyhow::Result<Vec<i64>>,
) -> Vec<i64> {
    outdated_ids_with_effects(
        db,
        None,
        |state, frosting, axis, _effects| fingerprint(state, frosting, axis),
        query,
    )
}

/// [`outdated_ids`] for a picture whose fingerprint also names the render physics a design
/// states in the library (`machine` is `Some`): the surface finish and abraded edge the vault
/// keeps in its side tables ([`EffectsKey::of_stored`], with the user's faceting machine).
/// `fingerprint` receives that key as its fourth argument; a design that states none gets
/// [`EffectsKey::NONE`], and so the text it had before the effects existed. With `machine`
/// `None` no effects are read and the key is always `NONE`.
pub fn outdated_ids_with_effects(
    db: &Database,
    machine: Option<&FacetingMachine>,
    fingerprint: impl Fn(&PreviewMaterialState, u64, Option<AxisKey>, EffectsKey) -> String,
    query: impl Fn(&Database, &dyn Fn(&PreviewMaterialState) -> String) -> anyhow::Result<Vec<i64>>,
) -> Vec<i64> {
    let polished_run =
        query(db, &|state| fingerprint(state, 0, None, EffectsKey::NONE)).unwrap_or_default();

    // One query each for every frosted design and every design with an axis of its own (an
    // entry the scans skip is simply never listed by a run, so it needs no filtering here).
    // A failed read lists nothing.
    let mut special: BTreeMap<i64, DesignState> = BTreeMap::new();
    for (entry_id, rows) in db.all_diagram_frosting().unwrap_or_default() {
        let hash = frosting_hash(&rows);
        if hash != 0 {
            special.entry(entry_id).or_default().0 = hash;
        }
    }
    for (entry_id, axis) in db.all_preview_axes().unwrap_or_default() {
        special.entry(entry_id).or_default().1 = Some(AxisKey::from_preview(axis));
    }
    if let Some(machine) = machine {
        for (entry_id, finish) in db.all_diagram_surface_finish().unwrap_or_default() {
            let key = EffectsKey::of_stored(&finish, machine);
            if key != EffectsKey::NONE {
                special.entry(entry_id).or_default().2 = key;
            }
        }
        // ... and the inclusion and play-of-color choice the design file states.
        for (entry_id, text) in db.all_diagram_inclusion_effects().unwrap_or_default() {
            let slot = &mut special.entry(entry_id).or_default().2;
            *slot = slot.with_stored_inclusions(Some(&text));
        }
    }
    if special.is_empty() {
        return polished_run;
    }

    let mut by_state: BTreeMap<StateKey, (DesignState, BTreeSet<i64>)> = BTreeMap::new();
    for (entry_id, state) in &special {
        by_state
            .entry((state.0, state.1.map(AxisKey::bits), state.2))
            .or_insert_with(|| (*state, BTreeSet::new()))
            .1
            .insert(*entry_id);
    }

    let mut outdated: BTreeSet<i64> = polished_run
        .into_iter()
        .filter(|id| !special.contains_key(id))
        .collect();
    for ((hash, axis, effects), ids) in by_state.values() {
        let run =
            query(db, &|state| fingerprint(state, *hash, *axis, *effects)).unwrap_or_default();
        outdated.extend(run.into_iter().filter(|id| ids.contains(id)));
    }
    outdated.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::preview_render::{CacheKind, FingerprintMaterial, cache_fingerprint};
    use pleochroma_vault::model::{entry::FacetingDiagramEntry, frosting::StoredTierFrosting};

    const PREVIEW: CacheKind = CacheKind::Preview {
        size: 160,
        spp: 4,
        max_bounces: 12,
    };

    fn entry(db: &Database, name: &str) -> i64 {
        db.save_diagram_entry(
            &FacetingDiagramEntry {
                title: name.to_string(),
                url: format!("local://{name}.asc"),
                design_id: String::new(),
            },
            "local-import",
        )
        .expect("the entry saves")
    }

    /// Stores a preview pair for `entry_id` under the fingerprint of a design with `frosting`.
    fn store_preview(db: &Database, entry_id: i64, frosting: u64) {
        let state = PreviewMaterialState::default();
        let fingerprint = cache_fingerprint(
            PREVIEW,
            FingerprintMaterial::from_state(&state).with_frosting(frosting),
        );
        let updated_at = db.entry_updated_at(entry_id).expect("the stamp reads");
        assert!(
            db.save_preview_images(
                entry_id,
                Some(b"front".as_slice()),
                Some(b"top".as_slice()),
                1,
                &fingerprint,
                updated_at,
            )
            .expect("the images save")
        );
    }

    fn scan(db: &Database) -> Vec<i64> {
        outdated_ids(
            db,
            |state, frosting, design_axis| {
                cache_fingerprint(
                    PREVIEW,
                    FingerprintMaterial::from_state(state)
                        .with_frosting(frosting)
                        .with_axis(design_axis),
                )
            },
            |db, fingerprint| db.entry_ids_missing_previews(fingerprint),
        )
    }

    /// A frosted design does not reuse an unfrosted cached preview, one stored under its own
    /// frosting is current, a different frosting is outdated again, and polished designs are
    /// judged exactly as before.
    #[test]
    fn a_frosted_design_is_outdated_until_its_preview_names_its_frosting() {
        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let polished = entry(&db, "polished");
        let frosted = entry(&db, "frosted");
        store_preview(&db, polished, 0);
        store_preview(&db, frosted, 0);
        assert_eq!(scan(&db), Vec::<i64>::new());

        let rows = [StoredTierFrosting::whole(1)];
        db.set_diagram_frosting(frosted, &rows)
            .expect("frosting stores");
        let hash = stored_frosting_hash(&db, frosted);
        assert_ne!(hash, 0);
        assert_eq!(
            scan(&db),
            vec![frosted],
            "the unfrosted picture of a frosted design is outdated"
        );

        store_preview(&db, frosted, hash);
        assert_eq!(
            scan(&db),
            Vec::<i64>::new(),
            "stored under its own frosting"
        );

        db.set_diagram_frosting(frosted, &[StoredTierFrosting::whole(2)])
            .expect("frosting stores");
        assert_eq!(
            scan(&db),
            vec![frosted],
            "another frosting is outdated again"
        );

        db.set_diagram_frosting(frosted, &[])
            .expect("frosting clears");
        assert_eq!(
            scan(&db),
            vec![frosted],
            "a picture stored frosted is outdated once the frosting is gone"
        );
        store_preview(&db, frosted, 0);
        assert_eq!(scan(&db), Vec::<i64>::new());
    }

    /// Stores a preview pair for `entry_id` under the fingerprint of a design drawn with
    /// `axis`.
    fn store_preview_with_axis(db: &Database, entry_id: i64, axis: Option<AxisKey>) {
        let state = PreviewMaterialState::default();
        let fingerprint = cache_fingerprint(
            PREVIEW,
            FingerprintMaterial::from_state(&state).with_axis(axis),
        );
        let updated_at = db.entry_updated_at(entry_id).expect("the stamp reads");
        assert!(
            db.save_preview_images(
                entry_id,
                Some(b"front".as_slice()),
                Some(b"top".as_slice()),
                1,
                &fingerprint,
                updated_at,
            )
            .expect("the images save")
        );
    }

    /// A design whose own file sets the crystal axis is outdated until its preview names that
    /// axis, goes outdated again when only the axis changes, and is current again once the
    /// override is off and the preview is stored without it; a design with no axis of its own
    /// is judged exactly as before throughout, and the texts of the two never collide.
    #[test]
    fn a_designs_own_crystal_axis_is_part_of_its_fingerprint() {
        use pleochroma_vault::model::preview::PreviewAxis;

        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let plain = entry(&db, "plain");
        let oriented = entry(&db, "oriented");
        store_preview_with_axis(&db, plain, None);
        store_preview_with_axis(&db, oriented, None);
        assert_eq!(scan(&db), Vec::<i64>::new());

        let axis = PreviewAxis {
            tilt_deg: 30.0,
            azimuth_deg: 120.0,
            second_deg: None,
        };
        db.set_preview_axis(oriented, Some(axis)).expect("stores");
        assert_eq!(
            scan(&db),
            vec![oriented],
            "the picture drawn without the axis is outdated"
        );
        assert_eq!(
            effective_axis_key(&db, oriented, None),
            Some(AxisKey::from_preview(axis))
        );
        assert_eq!(effective_axis_key(&db, plain, None), None);

        store_preview_with_axis(&db, oriented, effective_axis_key(&db, oriented, None));
        assert_eq!(scan(&db), Vec::<i64>::new(), "stored under its own axis");

        db.set_preview_axis(
            oriented,
            Some(PreviewAxis {
                tilt_deg: 31.0,
                ..axis
            }),
        )
        .expect("stores");
        assert_eq!(scan(&db), vec![oriented], "another axis is outdated again");

        db.set_preview_axis(oriented, None).expect("clears");
        assert_eq!(
            scan(&db),
            vec![oriented],
            "a picture stored with an axis is outdated once the override is off"
        );
        store_preview_with_axis(&db, oriented, None);
        assert_eq!(scan(&db), Vec::<i64>::new());
    }

    /// The default axes of the custom materials that store one are read by name, case
    /// ignored; a material with only one of the two angles, or none, is not listed.
    #[test]
    fn the_axis_table_lists_only_materials_with_a_stored_default_axis() {
        use pleochroma_vault::db::sqlite::CustomMaterialParams;

        let db = Database::new(Some(":memory:")).expect("in-memory database opens");
        let save = |name: &str, tilt: Option<f32>, azimuth: Option<f32>| {
            db.save_custom_material(&CustomMaterialParams {
                name,
                refractive_index: 1.6,
                dispersion: 0.02,
                birefringence: 0.01,
                absorption_rgb: [0.0; 3],
                crystal_system: None,
                optical_character: None,
                biaxial_delta_beta_alpha: None,
                per_axis_dispersion_json: None,
                specific_gravity: None,
                color_recipe_json: None,
                dispersion_model_json: None,
                absorption_bands_json: None,
                c_axis_tilt_deg: tilt,
                c_axis_azimuth_deg: azimuth,
                c_axis_second_azimuth_deg: None,
                inclusion_families_json: None,
                structure_json: None,
            })
            .expect("the material saves");
        };
        save("Oriented", Some(30.0), Some(120.0));
        save("Plain", None, None);
        save("Half Set", Some(30.0), None);

        let table = axis_key_table(&db);
        assert_eq!(table.len(), 1, "{table:?}");
        let key = axis_key_in(&table, Some("ORIENTED")).expect("matched ignoring case");
        assert_eq!(
            (key.tilt_deg, key.azimuth_deg, key.second_deg),
            (30.0, 120.0, None)
        );
        assert_eq!(axis_key_in(&table, Some("Plain")), None);
        assert_eq!(axis_key_in(&table, Some("Half Set")), None);
        assert_eq!(axis_key_in(&table, None), None);
        assert_eq!(stored_axis_key(&db, Some("oriented")), Some(key));
        assert_eq!(stored_axis_key(&db, None), None);
    }
}
