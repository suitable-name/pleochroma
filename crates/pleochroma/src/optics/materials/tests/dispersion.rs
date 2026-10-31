//! Dispersion regression tests: Abbe numbers, `n_d` at the sodium D line, physical
//! bounds at the sampled band edges, and the wavelength-dependent extraordinary
//! index.

use crate::optics::materials::{GemMaterial, OpticalCharacter};

/// Verifies every built-in material's computed Abbe number
/// `V_d = (n_d - 1) / (n_F - n_C)` against a trusted-source reference value, so a
/// future mistyped coefficient (Sellmeier or Cauchy) gets caught instead of
/// silently shipping -- see each material's own doc comment in `all_materials` for
/// the primary/reputable source and the derivation of its target Abbe number.
///
/// Source priority for these targets (re-verified against trusted online sources,
/// which now OUTRANK `GEMSTONE_RENDERING_BLUEPRINT.md` -- the document is used only
/// as a tiebreaker/corroboration, never as the primary authority, because it is a
/// research compilation whose own "Fire Delta n(F-C)" column demonstrably mixes
/// Fraunhofer B-G and F-C conventions row-to-row, e.g. Quartz lists 0.013, the
/// well-known B-G figure, in a column labelled F-C, contradicting that same row's
/// own `V_d`):
///   1. Primary optical measurements (Sellmeier/Cauchy fits from refractiveindex.info
///      and the papers it cites).
///   2. Reputable gemological references (GIA, Mindat, Handbook of Mineralogy,
///      International Gem Society), corroborated by 2+ independent sources where
///      possible, with their "dispersion" figures explicitly treated as Fraunhofer
///      B-G (never plugged directly into an F-C slot).
///   3. The document, as a tiebreaker only.
///
/// Two confidence tiers, reflected in the tolerance:
///   - TIGHT (0.5): Diamond, Sapphire, Ruby, Quartz, Spinel, Cubic Zirconia,
///     Alexandrite, Synthetic Moissanite, GGG -- dispersion coefficients transcribed
///     directly from a primary optical-constants paper (Peter 1923; Malitson &
///     Dodge 1972; Ghosh 1999; Tropf & Thomas 1991; Wood & Nassau 1982; Walling
///     1980; Wang 2013 corroborated by Singh 1971; Wood & Nassau 1990 for GGG), target computed from that exact
///     published formula. Alexandrite and Moissanite CONTRADICT the document here
///     (68.0 -> 73.90 and 21.5 -> 25.94 respectively) -- per the source priority
///     above the primary measurement wins; see each entry's comment in
///     `all_materials` for the two-source corroboration.
///   - LOWER CONFIDENCE (2.5-4.0, roughly 5-12% relative): Zircon, Topaz,
///     Tourmaline, Tanzanite, Emerald -- no primary Sellmeier/Cauchy fit exists in
///     the optics literature for these species (confirmed absent from
///     refractiveindex.info's database), so `n_d` is taken from 2+ corroborating
///     reputable gemological sources and the dispersion shape is a 2-parameter
///     Cauchy fit solved from that `n_d` and a Delta n(F-C) obtained by converting
///     each species' well-corroborated gemological B-G "dispersion" figure via a
///     B-G->F-C ratio (0.579, range 0.569-0.587) computed directly from the 8
///     genuine primary Sellmeier curves above -- not from cross-comparing
///     gemological tables against each other, which is how the earlier 0.591
///     factor was derived. All five CONTRADICT the document (Zircon 28.0 -> 40.96, Topaz 64.0 ->
///     76.83, Tourmaline 55.0 -> 64.58, Tanzanite 45.0 -> 39.85 (after the 2026-10-26 re-basing), Emerald's estimate
///     shifts modestly from 69.99 to 70.93 with the refined ratio). Zircon's `n_d`
///     also carried an unrelated, unsourced error, corrected to 1.925 from
///     1.956878 (see its entry's comment) -- critical-angle-relevant since `n_d`
///     drives it directly.
#[test]
fn builtin_material_abbe_numbers_match_published_values() {
    // (name, sourced Abbe number V_d, tolerance)
    let expected: &[(&str, f32, f32)] = &[
        // -- tight tier: primary Sellmeier source for both n_d and V_d --
        ("Diamond", 55.27, 0.5),
        ("Sapphire", 72.27, 0.5),
        ("Ruby", 72.27, 0.5),
        ("Quartz", 69.65, 0.5),
        ("Spinel", 60.63, 0.5),
        ("Cubic Zirconia", 33.52, 0.5),
        ("Alexandrite", 73.90, 0.5),
        ("Synthetic Moissanite", 25.94, 0.5),
        // Wood & Nassau 1990 Sellmeier: V_d = 0.97014 / 0.02546 = 38.11
        ("GGG", 38.11, 0.5),
        // -- lower-confidence tier: no primary fit exists; n_d from corroborated
        // gemological sources, dispersion shape from a gemological B-G figure
        // converted via the physically-derived 0.579 ratio -- see each entry's
        // comment in `all_materials` --
        ("Zircon", 40.96, 3.0),
        ("Topaz", 76.83, 4.0),
        ("Tourmaline", 64.58, 3.5),
        // 2026-10-26: base curve re-based to Hurlbut's n_beta 1.6943, B unchanged:
        // V_d = 0.6943 / 0.017422 = 39.85 (was 40.21)
        ("Tanzanite", 39.85, 3.0),
        ("Emerald", 70.93, 3.5),
    ];

    for &(name, published_abbe, tol) in expected {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let n_f = material.dispersion.evaluate(486.1);
        let n_d = material.dispersion.evaluate(589.3);
        let n_c = material.dispersion.evaluate(656.3);
        let dn = n_f - n_c;
        assert!(
            dn > 0.0,
            "{name}: Delta n(F-C) must be positive (normal dispersion), got {dn}"
        );
        let computed_abbe = (n_d - 1.0) / dn;
        assert!(
            (computed_abbe - published_abbe).abs() <= tol,
            "{name}: computed Abbe V_d={computed_abbe:.2} (n_d={n_d:.5}, n_F={n_f:.5}, n_C={n_c:.5}) \
                 does not match published V_d={published_abbe:.2} within tolerance {tol} -- check the \
                 dispersion coefficients against the source cited in all_materials()"
        );
    }
}
/// Companion to `builtin_material_abbe_numbers_match_published_values`: pins down
/// `n_d` at the sodium D line (589.3nm) for every re-sourced material below, so a
/// future coefficient edit that silently shifts `n_d` gets caught. `n_d` drives
/// the critical angle and therefore brilliance/windowing/extinction, so any
/// intentional change here is significant downstream and must be deliberate, not
/// an accident of refitting the dispersion shape.
///
/// For four of the six re-sourced materials (Synthetic Moissanite, Topaz,
/// Tourmaline, Alexandrite), `n_d` is unchanged from before -- only the
/// dispersion shape (`V_d`) moved, per
/// `builtin_material_abbe_numbers_match_published_values`. Tanzanite's `n_d` DID
/// change on 2026-10-26, from 1.700858 to 1.6943 (re-based to Hurlbut's measured
/// `n_beta`, American Mineralogist 54, 702 (1969): -0.0066, above the ~0.002
/// critical-angle threshold).
///
/// Zircon is the exception, and the one material in this file whose `n_d`
/// DID change materially: from 1.956878 to 1.925, a -0.0319 shift, far above
/// the ~0.002 threshold at which critical-angle-driven behaviour (brilliance,
/// windowing, extinction) is expected to move. The old value was never actually
/// sourced from anywhere (not the document, which gives `n_o=1.9250`, nor any
/// external reference found); 1.925 is corroborated by the Handbook of Mineralogy,
/// International Gem Society, and the document itself. See Zircon's comment in
/// `all_materials` for the full derivation. Any test elsewhere in the workspace
/// that hardcodes zircon's brilliance/windowing/pose behaviour will need
/// re-stabilising against this new `n_d`.
#[test]
fn builtin_material_n_d_matches_sourced_values() {
    // (name, sourced n_d, tolerance)
    let expected_n_d: &[(&str, f32, f32)] = &[
        ("Zircon", 1.925, 1e-4),
        ("Synthetic Moissanite", 2.647_434, 1e-4),
        ("Topaz", 1.627_178, 1e-4),
        ("Tourmaline", 1.639_405, 1e-4),
        ("Tanzanite", 1.694_3, 1e-4),
        ("Alexandrite", 1.742_73, 1e-4),
    ];

    for &(name, expected, tol) in expected_n_d {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let n_d = material.dispersion.evaluate(589.3);
        assert!(
            (n_d - expected).abs() < tol,
            "{name}: n_d={n_d:.6} does not match the sourced value {expected:.6} -- check the \
                 dispersion coefficients against the source cited in all_materials(); if this is a \
                 deliberate re-sourcing, update this test's expected value and flag the n_d change \
                 in the task report if it moves by more than ~0.002 (critical-angle significance)"
        );
    }
}
/// Every built-in material's dispersion curve (Sellmeier or Cauchy) must
/// evaluate to a finite, physically-sane (`n >= 1.0`) index at BOTH ends of this
/// renderer's actual sampled visible band (380nm violet, 780nm red) -- not just
/// near the sodium D line every other dispersion test in this file checks. Most
/// Cauchy fits here are only solved/verified near D and the F/C Fraunhofer lines
/// (486-656nm); this pins the `.max(1.0)` floor added to
/// `DispersionModel::Cauchy::evaluate` (see that variant's own doc comment) so an
/// out-of-fit-range extrapolation can never silently produce `n < 1` or a NaN.
#[test]
fn every_builtin_dispersion_curve_stays_physical_at_the_sampled_band_edges() {
    for material in GemMaterial::all_materials() {
        for lambda_nm in [380.0f32, 780.0] {
            let n = material.dispersion.evaluate(lambda_nm);
            assert!(
                n.is_finite(),
                "{}: dispersion.evaluate({lambda_nm}) must be finite, got {n}",
                material.name
            );
            assert!(
                n >= 1.0,
                "{}: dispersion.evaluate({lambda_nm}) = {n} must be >= 1.0 \
                     (physically-impossible index)",
                material.name
            );
            // The extraordinary ray (when a genuine independent curve is
            // present, or the constant-offset fallback otherwise) must be
            // equally well-behaved.
            if material.optical_character == OpticalCharacter::UniaxialPositive
                || material.optical_character == OpticalCharacter::UniaxialNegative
            {
                let n_e = material.extraordinary_index_at(lambda_nm, n);
                assert!(
                    n_e.is_finite() && n_e >= 1.0,
                    "{}: extraordinary_index_at({lambda_nm}, {n}) = {n_e} must be \
                         finite and >= 1.0",
                    material.name
                );
            }
        }
    }
}
/// Every new built-in must resolve by its own exact name
/// (the same `by_name` round-trip `by_name_round_trips_every_builtin_material`
/// already covers exhaustively -- this test's real job is the tolerance check
/// below) and its `n(589.3nm)` must match the target table below to within
/// 0.002.
#[test]
fn m4_new_species_match_their_target_n_d_within_tolerance() {
    // (name, target n_D, tolerance)
    let expected: &[(&str, f32, f32)] = &[
        ("Aquamarine", 1.577, 0.002),
        ("Morganite", 1.577, 0.002),
        ("Chrysoberyl (Yellow)", 1.746, 0.002),
        ("Amethyst", 1.544, 0.001),
        ("Citrine", 1.544, 0.001),
        ("Pyrope Garnet", 1.714, 0.002),
        ("Almandine Garnet", 1.790, 0.002),
        ("Spessartine Garnet", 1.800, 0.002),
        ("Color-Change Garnet (Pyrope-Spessartine)", 1.760, 0.001),
        ("Grossular Garnet (Tsavorite)", 1.734, 0.002),
        ("Andradite Garnet (Demantoid)", 1.887, 0.002),
        // base curve = n_beta
        ("Peridot", 1.669, 0.002),
        ("YAG", 1.833, 0.002),
        ("Benitoite", 1.757, 0.002),
        ("Andalusite", 1.634, 0.002),
        ("Opal", 1.45, 0.002),
        ("Glass (N-BK7)", 1.5168, 0.002),
        ("Glass (F2)", 1.620, 0.002),
    ];

    for &(name, target_n_d, tol) in expected {
        let material =
            GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must resolve via by_name"));
        assert_eq!(material.name, name, "{name} must resolve to itself exactly");
        let n_d = material.dispersion.evaluate(589.3);
        assert!(
            (n_d - target_n_d).abs() <= tol,
            "{name}: n_d={n_d:.5} does not match target {target_n_d} within {tol}"
        );
    }

    // GGG is checked separately and tighter: its Sellmeier is Wood & Nassau (1990)
    // Table I, whose D-line index is the paper's n_D = 1.97014.
    let ggg = GemMaterial::by_name("GGG").expect("GGG must resolve via by_name");
    let ggg_n_d = ggg.dispersion.evaluate(589.3);
    assert!(
        (ggg_n_d - 1.970_14).abs() <= 5e-4,
        "GGG: n_d={ggg_n_d:.5} does not match Wood & Nassau's 1.97014 within 5e-4"
    );
}

/// GGG's stored dispersion is Wood & Nassau's (Appl. Opt. 29, 3704, 1990) three-term
/// Sellmeier from their Table I (average of four crystals). The F-line value is the
/// formula's 1.98833: the paper's printed Table II `n_F` (1.98337) is inconsistent with
/// its own coefficients, `n_D` and `n_C` by exactly 0.005 (a digit slip), so the coefficients
/// are the reference. The He-Ne value 1.96515 is the paper's own printed one.
#[test]
fn ggg_dispersion_is_wood_and_nassaus() {
    let ggg = GemMaterial::by_name("GGG").expect("GGG must resolve via by_name");
    for (lambda_nm, expected) in [(486.13, 1.988_33), (656.27, 1.962_87), (632.8, 1.965_15)] {
        let n = ggg.dispersion.evaluate(lambda_nm);
        assert!(
            (n - expected).abs() <= 2e-4,
            "GGG: n({lambda_nm}) = {n:.5}, Wood & Nassau give {expected:.5}"
        );
    }
}
/// Peridot's base Cauchy curve is `n_beta` (the biaxial convention), so its principal triplet
/// at the D line must be the tabulated Deer, Howie & Zussman one for Fo~90 olivine:
/// alpha 1.654, beta 1.669, gamma 1.690. Pins the earlier slip that stored `n_alpha` (1.654)
/// as the base curve and so produced 1.639 / 1.654 / 1.675.
#[test]
fn peridot_principal_triplet_is_the_tabulated_one() {
    let peridot = GemMaterial::by_name("Peridot").expect("Peridot must be a built-in material");
    let indicatrix = peridot
        .biaxial_indicatrix(589.3)
        .expect("Peridot must expose a biaxial indicatrix");
    assert!(
        (indicatrix.n_alpha - 1.654).abs() <= 1e-3,
        "Peridot n_alpha = {}",
        indicatrix.n_alpha
    );
    assert!(
        (indicatrix.n_beta - 1.669).abs() <= 1e-3,
        "Peridot n_beta = {}",
        indicatrix.n_beta
    );
    assert!(
        (indicatrix.n_gamma - 1.690).abs() <= 1e-3,
        "Peridot n_gamma = {}",
        indicatrix.n_gamma
    );
    assert_eq!(
        peridot.optical_character,
        OpticalCharacter::BiaxialPositive,
        "Peridot stays optically positive"
    );
}

/// Tanzanite's principal triplet at the D line is Hurlbut's measured one (Am. Min. 54,
/// 702, 1969: 1.6925 / 1.6943 / 1.7015 +-0.0001, minimum deviation, Na light), and the
/// optic axial angle computed from it, `tan^2 V_z = (1/a^2 - 1/b^2) / (1/b^2 - 1/g^2)`
/// = 0.00074 / 0.00294, is Hurlbut's 2V = 53 degrees (accepted here: 50 to 56).
#[test]
fn tanzanite_principal_triplet_is_hurlbuts() {
    let tanzanite =
        GemMaterial::by_name("Tanzanite").expect("Tanzanite must be a built-in material");
    let indicatrix = tanzanite
        .biaxial_indicatrix(589.3)
        .expect("Tanzanite must expose a biaxial indicatrix");
    for (name, got, expected) in [
        ("alpha", indicatrix.n_alpha, 1.6925),
        ("beta", indicatrix.n_beta, 1.6943),
        ("gamma", indicatrix.n_gamma, 1.7015),
    ] {
        assert!(
            (got - expected).abs() <= 2e-4,
            "Tanzanite n_{name} = {got:.5}, Hurlbut gives {expected:.4}"
        );
    }
    let inv_sq = |n: f32| 1.0 / f64::from(n).powi(2);
    let tan_sq_vz = (inv_sq(indicatrix.n_alpha) - inv_sq(indicatrix.n_beta))
        / (inv_sq(indicatrix.n_beta) - inv_sq(indicatrix.n_gamma));
    let two_v = 2.0 * tan_sq_vz.sqrt().atan().to_degrees();
    assert!(
        (50.0..=56.0).contains(&two_v),
        "Tanzanite 2V = {two_v:.1} degrees, Hurlbut measured 53"
    );
}

/// Quartz's genuine per-axis (Ghosh o/e) dispersion must make its
/// extraordinary-ray index actually VARY with wavelength in a way the old
/// constant-offset approximation could not (the offset `n_o(lambda) +
/// birefringence_delta` tracks `n_o`'s own curvature exactly, so `n_e(lambda) -
/// n_o(lambda)` is constant under the old model but need not be under a genuine
/// independent curve); a material with no `uniaxial_extraordinary_dispersion` set
/// (e.g. Zircon) must keep the exact old constant-delta behaviour, bit-identical.
#[test]
fn quartz_extraordinary_index_has_wavelength_dependent_birefringence_while_legacy_entries_stay_constant()
 {
    let quartz = GemMaterial::by_name("Quartz").expect("Quartz must resolve");
    assert!(
        quartz.uniaxial_extraordinary_dispersion.is_some(),
        "test premise: Quartz must carry a genuine per-axis e-ray dispersion curve"
    );

    let delta_at = |lambda_nm: f32| {
        let n_o = quartz.dispersion.evaluate(lambda_nm);
        let n_e = quartz.extraordinary_index_at(lambda_nm, n_o);
        n_e - n_o
    };
    let delta_380 = delta_at(380.0);
    let delta_780 = delta_at(780.0);
    assert!(
        (delta_380 - delta_780).abs() > 1e-4,
        "Quartz's n_e - n_o must genuinely vary across the visible band: \
             delta(380nm)={delta_380:.6}, delta(780nm)={delta_780:.6}"
    );

    // A legacy entry (no per-axis curve) must keep the OLD constant-delta
    // behaviour exactly: n_e - n_o == birefringence_delta at every wavelength.
    // (Sapphire was this premise's legacy entry until it gained the Malitson & Dodge
    // e-ray curve; Zircon has no published per-ray dispersion.)
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must resolve");
    assert!(
        zircon.uniaxial_extraordinary_dispersion.is_none(),
        "test premise: Zircon must NOT carry a per-axis e-ray dispersion curve"
    );
    for lambda_nm in [380.0f32, 589.3, 780.0] {
        let n_o = zircon.dispersion.evaluate(lambda_nm);
        let n_e = zircon.extraordinary_index_at(lambda_nm, n_o);
        assert!(
            (n_e - n_o - zircon.birefringence_delta).abs() < 1e-6,
            "Zircon (legacy entry) at {lambda_nm}nm: n_e - n_o must equal the \
                 constant birefringence_delta exactly"
        );
    }
}

/// Rutile's principal indices follow `DeVore` (1951): `n_o(D) = 2.6129`, `n_e(D) =
/// 2.9086`, with Fraunhofer `Delta n(F-C)` of 0.1636 (o) and 0.2072 (e) -- far below
/// the gemological B-G figure (~0.30) the earlier Cauchy fit mistook for F-C -- and the
/// stored `birefringence_delta` equals `n_e(D) - n_o(D)`.
#[test]
fn rutile_matches_devore_indices_and_dispersion() {
    let rutile = GemMaterial::by_name("Rutile").expect("Rutile must be a built-in material");
    let e_ray = rutile
        .uniaxial_extraordinary_dispersion
        .expect("Rutile must carry its own e-ray dispersion curve");

    let n_o = rutile.dispersion.evaluate(589.3);
    let n_e = e_ray.evaluate(589.3);
    assert!((n_o - 2.613).abs() <= 0.002, "n_o(D) = {n_o}");
    assert!((n_e - 2.909).abs() <= 0.002, "n_e(D) = {n_e}");

    let fc_o = rutile.dispersion.evaluate(486.1) - rutile.dispersion.evaluate(656.3);
    let fc_e = e_ray.evaluate(486.1) - e_ray.evaluate(656.3);
    assert!((fc_o - 0.164).abs() <= 0.003, "o-ray F-C = {fc_o}");
    assert!((fc_e - 0.207).abs() <= 0.003, "e-ray F-C = {fc_e}");

    assert!(
        (rutile.birefringence_delta - (n_e - n_o)).abs() <= 1e-3,
        "birefringence_delta {} must equal n_e(D) - n_o(D) = {}",
        rutile.birefringence_delta,
        n_e - n_o
    );
}
