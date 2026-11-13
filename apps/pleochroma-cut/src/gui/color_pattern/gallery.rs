//! The "Start From..." gallery: which typical patterns, templates and presets it lists, and the
//! thumbnails of their tiles.
//!
//! A thumbnail is the pattern as picking the tile would make it (with this window's axis and size
//! rule), drawn in this stone's swatches by `thumbs.rs`. The session caches thumbnails by tile id
//! and swatch key, so a changed color, width or light path draws them again and nothing else does.

use super::{
    colors::{SwatchStone, ZoneSwatch, pattern_swatches},
    edit,
    presets::gallery_presets,
    preview::raster_image,
    session::{Host, Session},
    thumbs::pattern_thumbnail,
};
use crate::{ColorPatternModel, PatternTile};
use pleochroma_cut_core::material::pattern::{ColorPattern, TemplateId, typical_patterns};
use slint::{ComponentHandle, Image, ModelRc, VecModel};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

/// The edge of a thumbnail, pixels.
const THUMB_PX: usize = 96;

/// The templates the gallery lists for the stone named `material_name`: every template except the
/// ones the stone's typical patterns already show (a typical pattern of the same label is the
/// stone's own version of it). A tile index of the Templates section is an index into this list.
pub(super) fn templates(material_name: &str) -> Vec<TemplateId> {
    let typical = typical_patterns(material_name);
    TemplateId::ALL
        .into_iter()
        .filter(|id| typical.iter().all(|entry| entry.label != id.label()))
        .collect()
}

/// The section title of the typical patterns ("Typical for Tourmaline"); empty when the species
/// has none.
pub(super) fn typical_section(material_name: &str) -> String {
    if typical_patterns(material_name).is_empty() {
        String::new()
    } else {
        format!("Typical for {}", material_name.trim())
    }
}

/// A key for the colors a thumbnail shows: it changes when any swatch, the stone width or the
/// light path does.
pub(super) fn swatch_key(swatches: &[ZoneSwatch], stone: &SwatchStone<'_>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stone.width_mm.to_bits().hash(&mut hasher);
    stone.path_mm.to_bits().hash(&mut hasher);
    for swatch in swatches {
        swatch.primary.hash(&mut hasher);
        swatch.across.hash(&mut hasher);
        swatch.beta.hash(&mut hasher);
        swatch.blend.hash(&mut hasher);
        swatch.dichroic.hash(&mut hasher);
    }
    hasher.finish()
}

/// The thumbnail of `pattern` for tile `id`, from the cache or drawn and kept.
fn thumbnail(
    thumbs: &mut HashMap<(String, u64), Image>,
    id: String,
    pattern: &ColorPattern,
    stone: &SwatchStone<'_>,
) -> Image {
    let key = (id, swatch_key(&pattern_swatches(pattern, stone), stone));
    if let Some(image) = thumbs.get(&key) {
        return image.clone();
    }
    let image = raster_image(&pattern_thumbnail(pattern, stone, THUMB_PX)).unwrap_or_default();
    thumbs.insert(key, image.clone());
    image
}

/// One tile of a section: its label and the pattern picking it makes.
struct Entry {
    id: String,
    label: String,
    pattern: ColorPattern,
}

/// The entries of the three sections, as picking each would make the pattern.
fn entries(session: &Session) -> [Vec<Entry>; 3] {
    let width = session.stone_width_mm;
    let typical = typical_patterns(&session.base_material.name)
        .iter()
        .enumerate()
        .map(|(at, entry)| Entry {
            id: format!("typical:{at}:{}", entry.label),
            label: entry.label.to_owned(),
            pattern: edit::with_typical(
                &session.pattern,
                entry,
                &session.own,
                &session.variants,
                width,
            ),
        })
        .collect();
    let templates = templates(&session.base_material.name)
        .into_iter()
        .map(|id| Entry {
            id: format!("template:{}", id.label()),
            label: id.label().to_owned(),
            pattern: edit::with_template(&session.pattern, id, width, session.rank),
        })
        .collect();
    let presets = gallery_presets(&session.saved_presets)
        .into_iter()
        .map(|preset| Entry {
            id: format!("preset:{}", preset.name),
            label: preset.name,
            pattern: edit::with_preset(&session.pattern, &preset.pattern, width),
        })
        .collect();
    [typical, templates, presets]
}

/// Fills the gallery's three tile lists (and the typical section's title) from the session.
pub(super) fn push(host: &Host) {
    let Ok(mut guard) = host.session.try_borrow_mut() else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    // Taken out so the session can be read while the tiles are drawn; put back below.
    let mut thumbs = std::mem::take(&mut session.thumbs);
    let stone = session.swatch_stone();
    let [typical, templates, presets] = entries(session);
    let mut tiles = |entries: Vec<Entry>, typical: bool| -> ModelRc<PatternTile> {
        ModelRc::new(VecModel::from(
            entries
                .into_iter()
                .map(|entry| PatternTile {
                    label: entry.label.as_str().into(),
                    image: thumbnail(&mut thumbs, entry.id, &entry.pattern, &stone),
                    typical,
                })
                .collect::<Vec<_>>(),
        ))
    };
    let (typical_tiles, template_tiles, preset_tiles) = (
        tiles(typical, true),
        tiles(templates, false),
        tiles(presets, false),
    );
    let section = typical_section(&session.base_material.name);
    session.thumbs = thumbs;
    let model = host.window.global::<ColorPatternModel>();
    model.set_typical_section(section.into());
    model.set_typical_tiles(typical_tiles);
    model.set_template_tiles(template_tiles);
    model.set_preset_tiles(preset_tiles);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternAxis, PatternColor, ZoneColor, template};

    const STONE: SwatchStone<'static> = SwatchStone {
        path_mm: 10.0,
        width_mm: 7.0,
        host: None,
    };

    #[test]
    fn a_template_the_stone_has_a_typical_version_of_is_listed_once() {
        let listed = templates("Tourmaline");
        let typical = typical_patterns("Tourmaline");
        assert!(!typical.is_empty());
        for entry in typical {
            assert!(
                listed.iter().all(|id| id.label() != entry.label),
                "{} is a typical pattern of Tourmaline",
                entry.label
            );
        }
        assert!(listed.contains(&TemplateId::Banded));
        // A stone without typical patterns lists every template.
        assert_eq!(templates("Diamond").len(), TemplateId::ALL.len());
        assert_eq!(typical_section("Diamond"), "");
        assert_eq!(typical_section(" Tourmaline "), "Typical for Tourmaline");
    }

    #[test]
    fn a_thumbnail_key_follows_the_colors_of_the_stone() {
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let first = swatch_key(&pattern_swatches(&pattern, &STONE), &STONE);
        assert_eq!(
            first,
            swatch_key(&pattern_swatches(&pattern, &STONE), &STONE)
        );
        pattern.zones[0].color = PatternColor::Single(ZoneColor::unsolved([30.0, 40.0, 200.0]));
        let second = swatch_key(&pattern_swatches(&pattern, &STONE), &STONE);
        assert_ne!(first, second);
        let wider = SwatchStone {
            width_mm: 9.0,
            ..STONE
        };
        assert_ne!(
            second,
            swatch_key(&pattern_swatches(&pattern, &wider), &wider)
        );
    }
}
