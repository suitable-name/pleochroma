use super::*;
use pleochroma::optics::{
    absorption::{AbsorptionTensor, legacy_rgb_bands},
    dispersion::DispersionModel,
};

// --- EditorMaterialLookup::specific_gravity ---

#[test]
fn specific_gravity_is_none_when_not_opted_in_and_the_name_is_not_a_builtin() {
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    assert_eq!(lookup.specific_gravity("My Garnet"), None);
}

#[test]
fn specific_gravity_resolves_a_builtin_name_even_without_opting_in() {
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    assert_eq!(lookup.specific_gravity("Diamond"), Some(3.52));
}

#[test]
fn specific_gravity_resolves_a_custom_material_once_opted_in() {
    let custom_list: [GemMaterial; 0] = [];
    let sg_entries = [("My Garnet".to_string(), 3.90)];
    let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
    assert_eq!(lookup.specific_gravity("my garnet"), Some(3.90));
}

/// A custom material's own recorded SG wins over a same-named built-in's table
/// figure -- the same custom-over-built-in precedence [`EditorMaterialLookup::
/// lookup`] already applies for the material itself.
#[test]
fn specific_gravity_prefers_a_custom_entry_over_a_same_named_builtin() {
    let custom_list: [GemMaterial; 0] = [];
    let sg_entries = [("Diamond".to_string(), 3.515)];
    let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
    assert_eq!(lookup.specific_gravity("Diamond"), Some(3.515));
}

#[test]
fn specific_gravity_falls_back_to_a_builtin_when_opted_in_but_the_name_is_not_custom() {
    let custom_list: [GemMaterial; 0] = [];
    let sg_entries = [("My Garnet".to_string(), 3.90)];
    let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
    assert_eq!(lookup.specific_gravity("Quartz"), Some(2.65));
    assert_eq!(lookup.specific_gravity("Not A Real Material"), None);
}

#[test]
fn lookup_prefers_a_custom_material_over_a_same_named_built_in() {
    let mut custom = GemMaterial::diamond();
    custom.name = "Diamond".to_string();
    custom.dispersion = DispersionModel::Cauchy {
        a: 9.0,
        b: 0.0,
        c: 0.0,
    };
    let custom_list = [custom];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let found = lookup.lookup("Diamond").unwrap();
    assert!((f64::from(found.dispersion.evaluate(589.3)) - 9.0).abs() < 1e-6);
}

#[test]
fn lookup_falls_back_to_a_built_in_when_no_custom_material_matches() {
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    assert!(lookup.lookup("Quartz").is_some());
    assert!(lookup.lookup("Not A Real Material").is_none());
}

/// The catalogue's own case: a library row records a refractive index and no
/// species, so this is what names the stone for its preview. The four values are the
/// owner's actual catalogue's four most common (1,789 / 571 / 195 / 115 of 3,098
/// designs), so a regression here would mis-render most of the library.
#[test]
fn material_for_refractive_index_names_the_catalogues_common_stones() {
    for (n_d, expected) in [
        (1.54, "Quartz"),
        (1.76, "Sapphire"),
        (1.72, "Spinel"),
        (2.16, "Cubic Zirconia"),
    ] {
        let (name, gem) = material_for_refractive_index(n_d)
            .unwrap_or_else(|| panic!("{n_d} must resolve to a built-in"));
        assert_eq!(name, expected, "n_d {n_d}");
        assert_eq!(gem.name, expected, "n_d {n_d}: the resolved material too");
    }
}

/// Refuses rather than rounding to the nearest preset. 1.90 sits between
/// Cubic Zirconia (2.16) and Tanzanite/Zircon, further than
/// `MATERIAL_MATCH_TOLERANCE` from any.
#[test]
fn material_for_refractive_index_refuses_when_nothing_is_close() {
    assert!(material_for_refractive_index(1.90).is_none());
}

/// A design with no material name (every untouched import) must trace as the
/// material `view::traced_material_for` picked, not as diamond (the fallback for
/// unnamed selections). Diamond's empty absorption bands caused every imported
/// design to render colorless regardless of its RI.
#[test]
fn traced_gem_material_honours_the_traced_name_for_an_unnamed_selection() {
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let gem = traced_gem_material("Sapphire", &MaterialSelection::none(), &lookup)
        .expect("Sapphire is a built-in");
    let expected = GemMaterial::by_name("Sapphire").expect("Sapphire is a built-in");
    assert_eq!(gem.name, expected.name);
    assert_eq!(gem.absorption, expected.absorption);
    assert!(
        !gem.absorption.o_ray.is_empty(),
        "test premise: Sapphire must carry real absorption bands, or this test cannot tell it apart from diamond"
    );
}

/// An RI override still flattens the dispersion -- the traced name decides WHICH
/// material, the override decides its index, exactly as `resolved_gem_material` does.
#[test]
fn traced_gem_material_applies_the_refractive_index_override() {
    let selection = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.66),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let gem = traced_gem_material("Quartz", &selection, &lookup).expect("Quartz is a built-in");
    assert!((f64::from(gem.dispersion.evaluate(400.0)) - 1.66).abs() < 1e-6);
    assert!((f64::from(gem.dispersion.evaluate(700.0)) - 1.66).abs() < 1e-6);
}

/// A body-color override reaches BOTH resolution paths -- the optimizer/tilt
/// curve's `resolved_gem_material` and the render's `traced_gem_material` -- and
/// changes only the absorption: the name (and so every SG/RI lookup by name) and
/// the dispersion stay sapphire's own.
#[test]
fn a_body_color_override_reaches_both_resolution_paths() {
    let yellow = [0.2f32, 0.4, 2.8];
    let selection = MaterialSelection {
        name: Some("Sapphire".to_string()),
        ..MaterialSelection::none()
    }
    .with_body_color(Some(yellow));
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let expected_absorption = AbsorptionTensor::isotropic(legacy_rgb_bands(yellow));
    let base = GemMaterial::sapphire();

    let resolved = resolved_gem_material(&selection, &lookup);
    let traced =
        traced_gem_material("Sapphire", &selection, &lookup).expect("Sapphire is a built-in");
    for gem in [&resolved, &traced] {
        assert_eq!(gem.name, "Sapphire");
        assert_eq!(gem.dispersion, base.dispersion);
        assert_eq!(gem.absorption, expected_absorption);
    }
    assert_eq!(
        resolved, traced,
        "the two paths must describe the same stone"
    );
}

/// With both overrides set, both apply: flat dispersion at the typed RI AND the
/// color's absorption.
#[test]
fn ri_and_body_color_overrides_compose() {
    let blue = [2.8f32, 1.2, 0.1];
    let selection = MaterialSelection {
        name: Some("Quartz".to_string()),
        refractive_index_override: Some(1.60),
        ..MaterialSelection::none()
    }
    .with_body_color(Some(blue));
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let gem = resolved_gem_material(&selection, &lookup);
    assert!((f64::from(gem.dispersion.evaluate(450.0)) - 1.60).abs() < 1e-6);
    assert_eq!(
        gem.absorption,
        AbsorptionTensor::isotropic(legacy_rgb_bands(blue))
    );
}

/// Refuses rather than substituting: an unresolvable name leaves
/// `RenderContext::material_override` unset, so the by-name lookup in
/// `bridge::render_thread::context::resolve_material` decides on its own instead of
/// being overridden by a material nobody asked for.
#[test]
fn traced_gem_material_refuses_an_unknown_name() {
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    assert!(traced_gem_material("Kryptonite", &MaterialSelection::none(), &lookup).is_none());
}

#[test]
fn resolved_gem_material_uses_the_real_dispersion_curve_with_no_override() {
    let selection = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let gem = resolved_gem_material(&selection, &lookup);
    let expected = GemMaterial::by_name("Quartz").unwrap();
    assert_eq!(gem.dispersion, expected.dispersion);
}

#[test]
fn resolved_gem_material_flattens_dispersion_to_the_override_value() {
    let selection = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.70),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    let custom_list: [GemMaterial; 0] = [];
    let lookup = EditorMaterialLookup::new(&custom_list);
    let gem = resolved_gem_material(&selection, &lookup);
    // Evaluated at several wavelengths -- a flat fit must agree everywhere,
    // not just at the sodium D line the override was authored against.
    for lambda in [450.0f32, 589.3, 650.0] {
        assert!((f64::from(gem.dispersion.evaluate(lambda)) - 1.70).abs() < 1e-4);
    }
    // Every other field (crystal system, absorption, c-axis) stays whatever
    // the resolved base material ("Diamond") already had -- only dispersion
    // is replaced.
    let base = GemMaterial::diamond();
    assert_eq!(gem.crystal_system, base.crystal_system);
    assert_eq!(gem.absorption, base.absorption);
}

/// A custom material built from Sellmeier coefficients keeps its curve through every
/// path a design's material resolves by (lookup, optimizer/tilt resolution, render
/// trace); only an RI override flattens it, by design.
#[test]
fn a_custom_dispersion_model_survives_every_resolution_path_until_an_ri_override() {
    let model = GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion;
    assert!(matches!(model, DispersionModel::Sellmeier3 { .. }));
    let custom = GemMaterial::new_custom_with_dispersion("My Glass", model, 0.0, [0.0; 3]);
    let custom_list = [custom];
    let lookup = EditorMaterialLookup::new(&custom_list);

    let found = lookup
        .lookup("my glass")
        .expect("custom materials match any case");
    assert_eq!(found.dispersion, model);

    let selection = MaterialSelection {
        name: Some("My Glass".to_string()),
        ..MaterialSelection::none()
    };
    assert_eq!(resolved_gem_material(&selection, &lookup).dispersion, model);
    let traced =
        traced_gem_material("My Glass", &selection, &lookup).expect("the custom name resolves");
    assert_eq!(traced.dispersion, model);

    // The documented exception: a typed RI override replaces the curve with a flat one
    // at that index.
    let overridden = MaterialSelection {
        refractive_index_override: Some(1.60),
        ..selection
    };
    let flat = resolved_gem_material(&overridden, &lookup);
    assert_ne!(flat.dispersion, model);
    for lambda in [450.0f32, 589.3, 700.0] {
        assert!((f64::from(flat.dispersion.evaluate(lambda)) - 1.60).abs() < 1e-4);
    }
}

/// At sapphire's own RI, the nearest-first list must start with Sapphire
/// itself (distance 0) -- the guess badge's own headline candidate.
#[test]
fn material_guess_candidates_starts_with_the_exact_match() {
    let sapphire_n_d = pleochroma_cut_core::built_in_refractive_index("Sapphire").unwrap();
    let candidates = material_guess_candidates(sapphire_n_d, MATERIAL_MATCH_TOLERANCE);
    assert_eq!(
        candidates.first().map(|(name, _)| name.as_str()),
        Some("Sapphire")
    );
}

/// An RI with nothing close enough returns an empty list, not a forced
/// match -- same refusal-over-substitution rule as
/// [`material_for_refractive_index_refuses_when_nothing_is_close`].
#[test]
fn material_guess_candidates_is_empty_when_nothing_is_close() {
    assert_eq!(
        material_guess_candidates(1.90, MATERIAL_MATCH_TOLERANCE),
        Vec::<(String, f64)>::new()
    );
}

#[test]
fn nearest_built_in_material_finds_quartz_for_its_own_ri() {
    let quartz_n_d = pleochroma_cut_core::built_in_refractive_index("Quartz").unwrap();
    let (name, ri) = nearest_built_in_material(quartz_n_d, 0.01).unwrap();
    assert_eq!(name, "Quartz");
    assert!((ri - quartz_n_d).abs() < 1e-9);
}

#[test]
fn nearest_built_in_material_returns_none_outside_tolerance() {
    // 1.9999 sits roughly halfway between Spinel (~1.727) and Zircon
    // (~1.923)/Tanzanite (~1.69) -- nothing in the table is genuinely close,
    // so a tight tolerance must find nothing rather than force a match.
    assert_eq!(nearest_built_in_material(1.9999, 0.001), None);
}

#[test]
fn nearest_built_in_material_picks_the_closest_when_two_are_within_tolerance() {
    // Diamond (~2.417) and Cubic Zirconia (~2.15-2.2ish) may both fall within
    // a generous tolerance of some intermediate n_d -- the closer one must
    // win, not merely list order.
    let diamond_n_d = pleochroma_cut_core::built_in_refractive_index("Diamond").unwrap();
    let (name, _) = nearest_built_in_material(diamond_n_d - 0.02, 0.5).unwrap();
    assert_eq!(name, "Diamond");
}

// --- Selection round trip: every `MaterialCatalogue` entry must resolve to
// a `GemMaterial` of the SAME name and RI on the render side ---

/// `EditorMaterialLookup::lookup` and `bridge::render_thread::context::
/// resolve_material` implement the EXACT same precedence: a custom material by
/// case-insensitive name first, then `GemMaterial::by_name`/a linear built-in
/// scan, unconditionally (see `EditorMaterialLookup::lookup`'s own doc comment
/// and `resolve_material`'s body) -- there is no difference in behaviour for a
/// name `MaterialCatalogue` actually lists to exercise here that a direct call
/// into `bridge::render_thread` would catch and this wouldn't. This is
/// therefore the same round trip a design's `MaterialSelection::name` takes
/// from "listed in a picker" to "traced in Live Render".
#[test]
fn every_catalogue_entry_resolves_to_a_gem_material_of_its_own_name_and_ri() {
    let mut custom = GemMaterial::diamond();
    custom.name = "My Custom Garnet".to_string();
    custom.dispersion = DispersionModel::Cauchy {
        a: 1.74,
        b: 0.0,
        c: 0.0,
    };
    let custom_list = [custom];
    let catalogue = pleochroma_cut_core::MaterialCatalogue::build(&custom_list);
    assert!(
        catalogue.len() > 30,
        "premise: the catalogue must cover every built-in, not just thirteen \
         common ones -- {} entries found",
        catalogue.len()
    );
    let lookup = EditorMaterialLookup::new(&custom_list);

    for entry in catalogue.entries() {
        let selection = MaterialSelection {
            name: Some(entry.name.clone()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        };
        let resolved = resolved_gem_material(&selection, &lookup);
        assert_eq!(
            resolved.name, entry.name,
            "catalogue entry {:?} resolved to the wrong material",
            entry.name
        );
        let resolved_ri = f64::from(resolved.dispersion.evaluate(589.3));
        assert!(
            (resolved_ri - entry.ri_d).abs() < 1e-6,
            "{}: catalogue said RI {}, resolved material's own dispersion says {}",
            entry.name,
            entry.ri_d,
            resolved_ri
        );
    }
}

/// The render-path counterpart of the round trip above: `traced_gem_material`
/// (what `view::sync_viewport_material_link` actually calls) must agree too,
/// for every catalogue entry, with no RI override in play.
#[test]
fn every_catalogue_entry_traces_as_a_gem_material_of_its_own_name() {
    let catalogue = pleochroma_cut_core::MaterialCatalogue::build(&[]);
    let lookup = EditorMaterialLookup::new(&[]);
    for entry in catalogue.entries() {
        let gem = traced_gem_material(&entry.name, &MaterialSelection::none(), &lookup)
            .unwrap_or_else(|| panic!("{} must resolve via traced_gem_material", entry.name));
        assert_eq!(gem.name, entry.name);
    }
}

// --- traced_material_for (the missing-name case must refuse with a reason,
// never fall through to a by-name lookup that returns `None` -- no
// materials[0]/Diamond fallback -- and leave the viewport empty without an
// explanation) ---

fn fresh_design() -> Design {
    crate::EditorSession::fresh().design
}

#[test]
fn traced_material_for_names_a_resolvable_built_in_selection() {
    let mut design = fresh_design();
    design.material.name = Some("Sapphire".to_string());
    let (name, unresolved) = traced_material_for(&design, &[]);
    assert_eq!(name, "Sapphire");
    assert_eq!(unresolved, None);
}

#[test]
fn traced_material_for_names_a_resolvable_custom_selection() {
    let mut design = fresh_design();
    design.material.name = Some("My Garnet".to_string());
    let mut custom = GemMaterial::diamond();
    custom.name = "My Garnet".to_string();
    let (name, unresolved) = traced_material_for(&design, &[custom]);
    assert_eq!(name, "My Garnet");
    assert_eq!(unresolved, None);
}

/// The actual bug case this guards against: a design naming a material that
/// resolves through NEITHER the built-in table NOR the live custom-material
/// list (a deleted custom material, or a typo'd/hand-edited name) must refuse
/// -- not report itself resolved.
#[test]
fn traced_material_for_refuses_a_name_no_catalogue_resolves() {
    let mut design = fresh_design();
    design.material.name = Some("Deleted Custom Garnet".to_string());
    let (name, unresolved) = traced_material_for(&design, &[]);
    assert_eq!(name, "");
    let reason = unresolved.expect("an unresolvable name must refuse, not substitute");
    assert!(reason.contains("Deleted Custom Garnet"));
}

// --- applied_material_guess ---

#[test]
fn the_applied_guess_badge_shows_only_while_the_material_is_the_applied_one() {
    let mut design = fresh_design();
    design.material.name = Some("Sapphire".to_string());
    let (text, name, _) =
        applied_material_guess(&design, 1.76, Some("sapphire")).expect("badge shows");
    assert_eq!(name, "Sapphire");
    assert!(text.starts_with("Sapphire?"), "{text}");
    // A different pick retires it; nothing applied shows none.
    design.material.name = Some("Ruby".to_string());
    assert_eq!(
        applied_material_guess(&design, 1.76, Some("Sapphire")),
        None
    );
    assert_eq!(applied_material_guess(&design, 1.76, None), None);
}
