use pleochroma::optics::{
    materials::{CrystalSystem, GemMaterial},
    raytracer::LightingPreset,
};

use super::*;

fn spec_with(name: &str) -> SceneSpec {
    SceneSpec {
        material: MaterialSpec::catalogue(name),
        ..self_check_scene()
    }
}

#[test]
fn cubic_and_glass_materials_are_eligible() {
    for name in ["Diamond", "Spinel"] {
        assert_eq!(
            gpu_ineligibility_reason(&spec_with(name), 0, None, false),
            None,
            "{name} is cubic"
        );
    }
}

#[test]
fn birefringent_materials_are_refused() {
    for name in ["Zircon", "Topaz", "Sapphire"] {
        assert_eq!(
            gpu_ineligibility_reason(&spec_with(name), 0, None, false),
            Some(REASON_ANISOTROPIC),
            "{name} is birefringent"
        );
    }
}

#[test]
fn unknown_material_is_unresolved() {
    assert_eq!(
        gpu_ineligibility_reason(&spec_with("No Such Stone"), 0, None, false),
        Some(REASON_UNRESOLVED)
    );
}

#[test]
fn zoned_isotropic_material_is_refused() {
    let mut zoned = GemMaterial::ametrine();
    zoned.name = "Zoned Cubic".to_string();
    zoned.crystal_system = CrystalSystem::Cubic;
    zoned.birefringence_delta = 0.0;
    zoned.biaxial_delta_beta_alpha = None;
    assert!(zoned.zoning.is_some(), "the built-in Ametrine is zoned");
    let mut spec = spec_with("Zoned Cubic");
    spec.material.custom_materials = vec![zoned];
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, None, false),
        Some(REASON_ZONED)
    );
}

/// A stone with oriented inclusions needs a feature the browser kernel variant compiles out
/// (the Worker refuses every pass for it), so the page refuses it up front and never asks for
/// the kernel features of a stone the Worker would not trace (GPU-01).
#[test]
fn a_stone_with_oriented_inclusions_is_refused_before_any_kernel_feature_is_asked_for() {
    use pleochroma::optics::inclusions::{CrystalDir, CrystalFrame, Family, FamilyKind};

    let needle = Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    };
    let silky = GemMaterial::new_custom("Silky Cubic", 1.56, 0.012, 0.0, [0.0; 3])
        .with_inclusions(CrystalFrame::new(glam::Vec3::Y, None).resolve(&[needle]));
    assert_eq!(material_class(&silky), ISOTROPIC_CLASS);
    assert!(silky.scatters());
    let mut spec = spec_with("Silky Cubic");
    spec.material.custom_material_inclusions =
        crate::scene::MaterialInclusionsSpec::list_of(std::slice::from_ref(&silky));
    spec.material.custom_materials = vec![silky];
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, None, false),
        Some(REASON_ORIENTED)
    );
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&spec, 0, 0, None, false),
        Some(REASON_ORIENTED)
    );
    // The same stone without the silk is eligible.
    let mut plain = spec;
    plain.material.custom_material_inclusions = Vec::new();
    plain.material.custom_materials = vec![GemMaterial::new_custom(
        "Silky Cubic",
        1.56,
        0.012,
        0.0,
        [0.0; 3],
    )];
    assert_eq!(gpu_ineligibility_reason(&plain, 0, None, false), None);
}

#[test]
fn fluorescent_flag_is_refused() {
    assert_eq!(
        gpu_ineligibility_reason(&spec_with("Diamond"), 0, None, true),
        Some(REASON_FLUORESCENT)
    );
}

#[test]
fn uv_lamp_lighting_is_refused() {
    let uv = LightingPreset::ALL
        .iter()
        .copied()
        .find(|preset| preset.is_uv_lamp())
        .expect("the preset list has a UV lamp");
    let mut spec = spec_with("Diamond");
    spec.lighting = LightingSpec::new(uv, 1.0, 0.0, 0.0, Backdrop::AsLit, 16.0);
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, None, false),
        Some(REASON_UV_LAMP)
    );
}

#[test]
fn hdr_map_must_fit_the_storage_binding() {
    let mut spec = spec_with("Diamond");
    spec.hdr_id = Some(7);
    // 16 bytes per texel.
    assert_eq!(
        gpu_ineligibility_reason(&spec, 1_000 * 16 - 1, Some(1_000), false),
        Some(REASON_HDR_TOO_BIG)
    );
    assert_eq!(
        gpu_ineligibility_reason(&spec, 1_000 * 16, Some(1_000), false),
        None
    );
    // An unknown limit (the adapter is not up yet) or no map never blocks.
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, Some(u64::MAX), false),
        None
    );
    assert_eq!(gpu_ineligibility_reason(&spec, 16, None, false), None);
    // A map the scene does not name does not count.
    let plain = spec_with("Diamond");
    assert_eq!(
        gpu_ineligibility_reason(&plain, 16, Some(1_000), false),
        None
    );
}

/// Concave stones go to the GPU up to the kernels' tool limit and are the CPU's beyond it; the
/// reason names the limit and never hides a material reason.
#[test]
fn a_concave_stone_is_eligible_up_to_the_tool_limit() {
    use pleochroma::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;

    let diamond = spec_with("Diamond");
    for tools in [0, 1, GPU_MAX_TOOL_PRIMITIVES] {
        assert_eq!(
            gpu_ineligibility_reason_with_tools(&diamond, tools, 0, None, false),
            None,
            "{tools} tools"
        );
    }
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&diamond, GPU_MAX_TOOL_PRIMITIVES + 1, 0, None, false),
        Some(REASON_TOO_MANY_TOOLS)
    );
    // The material reasons keep their precedence.
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&spec_with("Zircon"), 1_000, 0, None, false),
        Some(REASON_ANISOTROPIC)
    );
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&diamond, 1, 0, None, true),
        Some(REASON_FLUORESCENT)
    );
}

/// Without tools the new entry point is the old one, for every reason the old one gives.
#[test]
fn the_tool_aware_reason_equals_the_plain_one_without_tools() {
    for name in ["Diamond", "Spinel", "Zircon", "Sapphire", "No Such Stone"] {
        let spec = spec_with(name);
        for fluorescent in [false, true] {
            assert_eq!(
                gpu_ineligibility_reason_with_tools(&spec, 0, 0, None, fluorescent),
                gpu_ineligibility_reason(&spec, 0, None, fluorescent),
                "{name}, fluorescent {fluorescent}"
            );
        }
    }
}

/// The number in the reason text is the kernels' limit.
#[test]
fn the_too_many_tools_reason_names_the_limit() {
    let limit = pleochroma::geometry::tool::GPU_MAX_TOOL_PRIMITIVES.to_string();
    assert_eq!(
        REASON_TOO_MANY_TOOLS.matches(limit.as_str()).count(),
        2,
        "{REASON_TOO_MANY_TOOLS}"
    );
}

#[test]
fn the_self_check_scene_is_eligible_and_small() {
    let scene = self_check_scene();
    assert_eq!((scene.width, scene.height), (16, 16));
    assert_eq!(gpu_ineligibility_reason(&scene, 0, None, false), None);
}

#[test]
fn fnv_matches_known_vectors() {
    assert_eq!(fnv1a_64(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a_64("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a_64("foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn chrome_major_is_read_from_chrome_chromium_and_edge() {
    let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.7390.55 Safari/537.36";
    let edge = format!("{chrome} Edg/141.0.3537.57");
    assert_eq!(chrome_major_from_user_agent(chrome), "141");
    assert_eq!(chrome_major_from_user_agent(&edge), "141");
    assert_eq!(
        chrome_major_from_user_agent("Mozilla/5.0 Firefox/130.0"),
        "0"
    );
    assert_eq!(chrome_major_from_user_agent("Chrome/abc.1"), "0");
    assert_eq!(chrome_major_from_user_agent(""), "0");
}

fn facts() -> AdapterFacts<'static> {
    AdapterFacts {
        vendor: "0000",
        architecture: "",
        device: "0000",
        description: "",
        backend: "BrowserWebGpu",
        max_storage_buffers_per_stage: 10,
        max_storage_binding: 2_147_483_644,
        max_buffer: 4_294_967_296,
    }
}

#[test]
fn adapter_key_is_stable_and_well_formed() {
    let key = adapter_key(&facts(), "141", 0x1234_u64);
    assert_eq!(key, adapter_key(&facts(), "141", 0x1234_u64));
    assert!(key.starts_with("pleochroma.admission."));
    assert_eq!(key.rsplit_once('.').map(|(_, version)| version), Some("v1"));
}

#[test]
fn adapter_key_separates_gpus_that_report_the_same_empty_identity() {
    let base = adapter_key(&facts(), "141", 1_u64);
    let other_limits = AdapterFacts {
        max_storage_binding: 134_217_728,
        ..facts()
    };
    let other_buffer = AdapterFacts {
        max_buffer: 268_435_456,
        ..facts()
    };
    let other_count = AdapterFacts {
        max_storage_buffers_per_stage: 8,
        ..facts()
    };
    let named = AdapterFacts {
        description: "NVIDIA RTX A6000",
        ..facts()
    };
    for different in [other_limits, other_buffer, other_count, named] {
        assert_ne!(adapter_key(&different, "141", 1_u64), base);
    }
}

#[test]
fn adapter_key_changes_with_browser_and_kernel_set() {
    let base = adapter_key(&facts(), "141", 1_u64);
    assert_ne!(adapter_key(&facts(), "142", 1_u64), base);
    assert_ne!(adapter_key(&facts(), "141", 2_u64), base);
}

#[test]
fn admission_state_serde_is_lowercase() {
    for (state, text) in [
        (AdmissionState::Unknown, "\"unknown\""),
        (AdmissionState::Probing, "\"probing\""),
        (AdmissionState::Ok, "\"ok\""),
        (AdmissionState::Failed, "\"failed\""),
    ] {
        assert_eq!(serde_json::to_string(&state).unwrap(), text);
        assert_eq!(serde_json::from_str::<AdmissionState>(text).unwrap(), state);
    }
    assert_eq!(AdmissionState::default(), AdmissionState::Unknown);
}

#[test]
fn admission_record_round_trips() {
    let mut per_kernel = BTreeMap::new();
    per_kernel.insert("Extend".to_string(), 12.5);
    per_kernel.insert("Surface".to_string(), 8.3);
    let record = AdmissionRecord {
        state: AdmissionState::Failed,
        per_kernel_ms: per_kernel,
        max_ms: 12.5,
        total_ms: 20.8,
        reason: Some("compile timeout".to_string()),
        when: "2026-10-16T12:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&record).unwrap();
    assert_eq!(
        serde_json::from_str::<AdmissionRecord>(&json).unwrap(),
        record
    );
}

fn record_json(state: AdmissionState) -> String {
    serde_json::to_string(&AdmissionRecord {
        state,
        ..AdmissionRecord::default()
    })
    .unwrap()
}

#[test]
fn probing_scan_finds_every_crash_marker_and_ignores_other_keys() {
    let ok = record_json(AdmissionState::Ok);
    let probing = record_json(AdmissionState::Probing);
    let entries = [
        ("pleochroma.settings.v1", probing.as_str()),
        ("pleochroma.admission.a.v1", ok.as_str()),
        ("pleochroma.admission.b.v1", probing.as_str()),
        ("pleochroma.admission.c.v1", "not json"),
        ("pleochroma.admission.d.v1", probing.as_str()),
    ];
    assert_eq!(
        find_probing_key(entries.iter().copied()).as_deref(),
        Some("pleochroma.admission.b.v1")
    );
    assert_eq!(
        probing_keys(entries.iter().copied()),
        vec![
            "pleochroma.admission.b.v1".to_string(),
            "pleochroma.admission.d.v1".to_string()
        ]
    );
}

#[test]
fn probing_scan_is_empty_without_markers() {
    let ok = record_json(AdmissionState::Ok);
    let failed = record_json(AdmissionState::Failed);
    let entries = [
        ("pleochroma.admission.a.v1", ok.as_str()),
        ("pleochroma.admission.b.v1", failed.as_str()),
    ];
    assert_eq!(find_probing_key(entries.iter().copied()), None);
    assert_eq!(probing_keys(entries.iter().copied()).len(), 0);
}

#[test]
fn kernel_order_has_no_duplicates() {
    let mut names = ISOTROPIC_BASE_KERNELS.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ISOTROPIC_BASE_KERNELS.len());
}

fn sorted_names(names: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    sorted.sort_unstable();
    sorted
}

/// Pins the shipped admission: only the isotropic class reaches the GPU in the browser. Adding a
/// class to `ADMITTED_CLASSES` is the owner's decision after the browser probes, and this test is
/// the one to change with it.
#[test]
fn only_the_isotropic_class_is_admitted_until_the_probes_pass() {
    assert_eq!(ADMITTED_CLASSES, &[ISOTROPIC_CLASS]);
    assert!(is_class_admitted(ISOTROPIC_CLASS));
    assert!(!is_class_admitted(UNIAXIAL_CLASS));
    assert!(!is_class_admitted(BIAXIAL_CLASS));
}

/// The class is the kind bits of the renderer's own `classify_material`.
#[test]
fn material_class_equals_the_renderers_own_classifier_for_every_built_in() {
    use pleochroma::renderer::gpu::frame::{classify_material, material_class as renderer_class};

    for material in GemMaterial::built_in() {
        assert_eq!(
            classify_material(material) & renderer_class::KIND_MASK,
            material_class(material),
            "{}",
            material.name
        );
    }
}

/// The GPU upload (`GpuGemMaterial::encode`) makes the same derivation as the classifier, so
/// every built-in material must agree with its flags.
#[test]
fn material_class_agrees_with_the_gpu_upload_for_every_built_in() {
    use pleochroma::renderer::buffers::GpuGemMaterial;

    let (mut isotropic, mut uniaxial, mut biaxial) = (0, 0, 0);
    for material in GemMaterial::built_in() {
        let encoded = GpuGemMaterial::encode(material);
        let expected = if encoded.dispersion.has_biaxial_delta != 0 {
            BIAXIAL_CLASS
        } else if encoded.dispersion.is_anisotropic != 0 {
            UNIAXIAL_CLASS
        } else {
            ISOTROPIC_CLASS
        };
        assert_eq!(material_class(material), expected, "{}", material.name);
        match expected {
            ISOTROPIC_CLASS => isotropic += 1,
            UNIAXIAL_CLASS => uniaxial += 1,
            _ => biaxial += 1,
        }
    }
    assert!(
        isotropic > 0 && uniaxial > 0 && biaxial > 0,
        "the built-ins cover all three classes ({isotropic}/{uniaxial}/{biaxial})"
    );
}

#[test]
fn scene_material_class_names_the_class_of_catalogue_stones() {
    assert_eq!(
        scene_material_class(&spec_with("Diamond")),
        Some(ISOTROPIC_CLASS)
    );
    assert_eq!(
        scene_material_class(&spec_with("Zircon")),
        Some(UNIAXIAL_CLASS)
    );
    let topaz = GemMaterial::built_in()
        .iter()
        .find(|material| material.biaxial_delta_beta_alpha.is_some())
        .expect("the built-ins have a biaxial stone");
    assert_eq!(
        scene_material_class(&spec_with(&topaz.name)),
        Some(BIAXIAL_CLASS)
    );
    assert_eq!(scene_material_class(&spec_with("No Such Stone")), None);
}

/// The kernel names the desktop wavefront builds for `class` and the three scene conditions,
/// sorted.
fn desktop_kernel_names(
    class: u32,
    frosted: bool,
    scatters: bool,
    hdr_scatter: bool,
) -> Vec<String> {
    use pleochroma::renderer::gpu::event_wavefront::kernels::kernels_for;

    let mut names: Vec<String> = kernels_for(class, frosted, scatters, hdr_scatter)
        .iter()
        .map(|kernel| format!("{kernel:?}"))
        .collect();
    names.sort_unstable();
    names
}

/// The names this crate would compile for a stone of `kind` that needs `features`, sorted:
/// the base set plus the kernels of each feature.
fn web_kernel_names(kind: u32, features: FeatureSet) -> Vec<String> {
    let mut names: Vec<&str> = kernels_for_class(kind).to_vec();
    for feature in features.iter() {
        names.extend_from_slice(feature.kernels());
    }
    sorted_names(&names)
}

/// The base set plus the kernels of the needed features name exactly the kernels the desktop
/// wavefront builds for that stone with the browser's pipeline variant, for every kind and
/// every combination of frosted facets, scattering and scattering under an HDR map. This is
/// what keeps the name tables from drifting away from `kernels_for`.
#[test]
fn base_set_plus_features_match_the_event_wavefront_kernel_selection() {
    for kind in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        let variant = browser_pipeline_class(kind);
        for (frosted, scatters, hdr_scatter) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, true, true),
            (true, true, false),
            (true, true, true),
        ] {
            let mut features = FeatureSet::EMPTY;
            if scatters {
                features = features.with(KernelFeature::Scattering);
            }
            if hdr_scatter {
                features = features.with(KernelFeature::ScatterNee);
            }
            if frosted {
                features = features.with(KernelFeature::Frosted);
            }
            assert_eq!(
                web_kernel_names(kind, features),
                desktop_kernel_names(variant, frosted, scatters, hdr_scatter),
                "class {kind}, frosted {frosted}, scatters {scatters}, hdr {hdr_scatter}"
            );
        }
    }
}

/// The browser variant keeps the scattering NEE on, so the feature kernels are never empty (an
/// empty feature would be compiled for nothing).
#[test]
fn every_feature_adds_kernels_the_base_set_lacks() {
    for kind in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        let variant = browser_pipeline_class(kind);
        let base = desktop_kernel_names(variant, false, false, false);
        assert_eq!(sorted_names(kernels_for_class(kind)), base, "class {kind}");
        for (feature, with) in [
            (
                KernelFeature::Scattering,
                desktop_kernel_names(variant, false, true, false),
            ),
            (
                KernelFeature::ScatterNee,
                desktop_kernel_names(variant, false, true, true),
            ),
            (
                KernelFeature::Frosted,
                desktop_kernel_names(variant, true, false, false),
            ),
        ] {
            assert!(!feature.kernels().is_empty(), "{feature:?}");
            for kernel in feature.kernels() {
                assert!(
                    !base.iter().any(|name| name.as_str() == *kernel),
                    "{kernel}"
                );
                assert!(with.iter().any(|name| name.as_str() == *kernel), "{kernel}");
            }
        }
    }
}

#[test]
fn base_lists_are_the_isotropic_set_plus_the_uniaxial_arms() {
    assert_eq!(kernels_for_class(ISOTROPIC_CLASS), ISOTROPIC_BASE_KERNELS);
    assert_eq!(kernels_for_class(BIAXIAL_CLASS), ISOTROPIC_BASE_KERNELS);
    assert_eq!(kernels_for_class(UNIAXIAL_CLASS), UNIAXIAL_BASE_KERNELS);
    assert_eq!(
        &UNIAXIAL_BASE_KERNELS[..ISOTROPIC_BASE_KERNELS.len()],
        ISOTROPIC_BASE_KERNELS
    );
    assert_eq!(
        &UNIAXIAL_BASE_KERNELS[ISOTROPIC_BASE_KERNELS.len()..],
        &["ArmUniEntry", "ArmUniEntryTransmit", "ArmUniInternal"]
    );
    let mut names = UNIAXIAL_BASE_KERNELS.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), UNIAXIAL_BASE_KERNELS.len());
    // An unknown class compiles the isotropic list.
    assert_eq!(kernels_for_class(0), ISOTROPIC_BASE_KERNELS);
    // The scattering, HDR scattering and frosted kernels are no longer in a class's list.
    for kind in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        for name in ["Scatter", "NeeHg", "ArmFrosted"] {
            assert!(!kernels_for_class(kind).contains(&name), "{name}");
        }
    }
}

/// Every class has a record of its own, and none is the key made before the compiled variant
/// existed: a record of the all-features pipeline is never read for the smaller one.
#[test]
fn class_keys_are_separate_and_leave_the_all_features_key_behind() {
    let isotropic = class_adapter_key(&facts(), "141", 7_u64, ISOTROPIC_CLASS);
    assert_ne!(isotropic, adapter_key(&facts(), "141", 7_u64));
    let uniaxial = class_adapter_key(&facts(), "141", 7_u64, UNIAXIAL_CLASS);
    let biaxial = class_adapter_key(&facts(), "141", 7_u64, BIAXIAL_CLASS);
    let ray = whitted_adapter_key(&facts(), "141", "7");
    let keys = [&isotropic, &uniaxial, &biaxial, &ray];
    for (i, a) in keys.iter().enumerate() {
        assert!(a.starts_with("pleochroma.admission."), "{a}");
        for b in &keys[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert_eq!(
        uniaxial,
        class_adapter_key(&facts(), "141", 7_u64, UNIAXIAL_CLASS)
    );
}

/// The constants this crate repeats are the renderer's own.
#[test]
fn browser_class_bits_are_the_renderers_feature_bits() {
    use pleochroma::renderer::gpu::frame::material_class as renderer_class;

    let variant = browser_pipeline_class(UNIAXIAL_CLASS);
    assert_eq!(
        variant,
        renderer_class::UNIAXIAL | renderer_class::NO_ORIENTED | renderer_class::NO_ZONED_ORIENTED
    );
    assert_eq!(
        browser_pipeline_class(ISOTROPIC_CLASS),
        renderer_class::ISOTROPIC
            | renderer_class::NO_ORIENTED
            | renderer_class::NO_ZONED_ORIENTED
            | renderer_class::NO_EXTRAORDINARY_DISPERSION
    );
    assert_eq!(
        browser_pipeline_class(BIAXIAL_CLASS),
        renderer_class::BIAXIAL
            | renderer_class::NO_ORIENTED
            | renderer_class::NO_ZONED_ORIENTED
            | renderer_class::NO_EXTRAORDINARY_DISPERSION
    );
    assert_eq!(class_kind(variant), UNIAXIAL_CLASS);
}

/// What the browser compiles per kind: oriented inclusions and zoning are always out, the
/// scattering NEE is always in, and only uniaxial keeps the extraordinary curve.
#[test]
fn the_browser_variant_compiles_out_only_what_a_browser_scene_cannot_reach() {
    use pleochroma::renderer::gpu::frame::material_class::pipeline_constants;

    for (class, extraordinary) in [
        (ISOTROPIC_CLASS, 0.0),
        (UNIAXIAL_CLASS, 1.0),
        (BIAXIAL_CLASS, 0.0),
    ] {
        let constants = pipeline_constants(browser_pipeline_class(class));
        assert_eq!(constants[0], ("MATERIAL_CLASS", f64::from(class)));
        assert_eq!(constants[1], ("ORIENTED_INCLUSIONS", 0.0), "class {class}");
        assert_eq!(constants[2], ("ZONED_ORIENTED", 0.0), "class {class}");
        assert_eq!(constants[3], ("SCATTER_NEE", 1.0), "class {class}");
        assert_eq!(
            constants[4],
            ("EXTRAORDINARY_DISPERSION", extraordinary),
            "class {class}"
        );
    }
}

#[test]
fn values_that_are_no_material_class_pass_through_the_browser_variant() {
    assert_eq!(browser_pipeline_class(0), 0);
    assert_eq!(browser_pipeline_class(WHITTED_CLASS), WHITTED_CLASS);
    assert_eq!(class_kind(WHITTED_CLASS), WHITTED_CLASS);
    assert_eq!(class_kind(0), 0);
    assert_eq!(browser_pass_class(0), None);
    // A value that already carries feature bits maps to the same variant as its kind.
    for kind in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        assert_eq!(
            browser_pipeline_class(browser_pipeline_class(kind)),
            browser_pipeline_class(kind)
        );
        assert_eq!(class_kind(browser_pipeline_class(kind)), kind);
    }
}

/// The pass class of every built-in stone is the variant that was compiled for its kind, never
/// another one; a stone is refused only when it has oriented inclusions or an extraordinary
/// curve outside the uniaxial class, which the variant compiled out.
#[test]
fn every_built_in_stone_traces_with_the_variant_that_was_compiled_for_its_kind() {
    use pleochroma::renderer::{buffers::GpuGemMaterial, gpu::frame::classify_material};

    let mut accepted = 0;
    for material in GemMaterial::built_in() {
        let kind = material_class(material);
        let encoded = GpuGemMaterial::encode(material);
        let reachable = material.oriented_inclusions().is_none()
            && (kind == UNIAXIAL_CLASS || encoded.has_extraordinary_dispersion == 0);
        let expected = reachable.then(|| browser_pipeline_class(kind));
        assert_eq!(
            browser_pass_class(classify_material(material)),
            expected,
            "{}",
            material.name
        );
        accepted += usize::from(reachable);
    }
    assert!(accepted > 0, "some built-in stone is reachable");
    for name in ["Diamond", "Zircon", "Quartz"] {
        let material = GemMaterial::by_name(name).expect("built-in material");
        assert!(
            browser_pass_class(classify_material(&material)).is_some(),
            "{name}"
        );
    }
}

/// A stone that needs a feature the browser variant compiled out is refused, not traced with
/// a pipeline that would silently drop the feature.
#[test]
fn a_stone_needing_a_compiled_out_feature_gets_no_pass_class() {
    use pleochroma::renderer::gpu::frame::material_class as c;

    // Oriented inclusions, zoned oriented inclusions, an extraordinary curve on an isotropic or
    // biaxial kind: each needs a feature the variant removed.
    assert_eq!(
        browser_pass_class(c::ISOTROPIC | c::NO_ZONED_ORIENTED),
        None
    );
    assert_eq!(browser_pass_class(c::ISOTROPIC | c::NO_SCATTER_NEE), None);
    assert_eq!(browser_pass_class(c::ISOTROPIC), None);
    assert_eq!(
        browser_pass_class(c::BIAXIAL | c::NO_ORIENTED | c::NO_ZONED_ORIENTED),
        None
    );
    // The same stones without those needs are accepted, and a uniaxial curve is kept.
    assert_eq!(
        browser_pass_class(c::UNIAXIAL | c::NO_ORIENTED | c::NO_ZONED_ORIENTED),
        Some(browser_pipeline_class(UNIAXIAL_CLASS))
    );
    assert_eq!(
        browser_pass_class(
            c::UNIAXIAL | c::NO_ORIENTED | c::NO_ZONED_ORIENTED | c::NO_EXTRAORDINARY_DISPERSION
        ),
        Some(browser_pipeline_class(UNIAXIAL_CLASS))
    );
    assert_eq!(
        browser_pass_class(
            c::BIAXIAL | c::NO_ORIENTED | c::NO_ZONED_ORIENTED | c::NO_EXTRAORDINARY_DISPERSION
        ),
        Some(browser_pipeline_class(BIAXIAL_CLASS))
    );
}

/// The kernel lists and the compiled mark follow the kind, so a feature-carrying value finds
/// the same list as its kind.
#[test]
fn kernel_lists_follow_the_kind_of_a_class_value() {
    assert_eq!(
        kernels_for_class(browser_pipeline_class(UNIAXIAL_CLASS)),
        UNIAXIAL_BASE_KERNELS
    );
    assert_eq!(
        kernels_for_class(browser_pipeline_class(ISOTROPIC_CLASS)),
        ISOTROPIC_BASE_KERNELS
    );
    assert_eq!(
        kernels_for_class(browser_pipeline_class(BIAXIAL_CLASS)),
        ISOTROPIC_BASE_KERNELS
    );
}

/// A changed compiled variant is a new record: the key carries the variant number.
#[test]
fn the_class_key_carries_the_compiled_variant() {
    let key = class_adapter_key(&facts(), "141", 7_u64, ISOTROPIC_CLASS);
    assert_eq!(
        key,
        adapter_key(&facts(), "141", format!("7:c1:p{BROWSER_PIPELINE_VARIANT}"))
    );
    assert_ne!(
        key,
        adapter_key(
            &facts(),
            "141",
            format!("7:c1:p{}", BROWSER_PIPELINE_VARIANT + 1)
        )
    );
}

#[test]
fn class_labels_name_the_three_classes() {
    assert_eq!(class_label(ISOTROPIC_CLASS), "isotropic");
    assert_eq!(class_label(UNIAXIAL_CLASS), "uniaxial");
    assert_eq!(class_label(BIAXIAL_CLASS), "biaxial");
}

#[test]
fn an_untested_class_waits_for_its_compile_but_isotropic_and_ray_never_do() {
    use pleochroma::renderer::engine::{EngineSpec, RenderEngine};

    let path = EngineSpec::default();
    let ray = EngineSpec {
        engine: RenderEngine::RayTraced,
        ..path
    };
    let zircon = spec_with("Zircon");
    let diamond = spec_with("Diamond");
    assert_eq!(
        class_untested_reason(path, &zircon, 0),
        Some(REASON_CLASS_NOT_TESTED)
    );
    assert_eq!(
        class_untested_reason(path, &zircon, class_bit(BIAXIAL_CLASS)),
        Some(REASON_CLASS_NOT_TESTED)
    );
    assert_eq!(
        class_untested_reason(path, &zircon, class_bit(UNIAXIAL_CLASS)),
        None
    );
    assert_eq!(class_untested_reason(path, &diamond, 0), None);
    assert_eq!(class_untested_reason(ray, &zircon, 0), None);
    assert_eq!(
        class_untested_reason(path, &spec_with("No Such Stone"), 0),
        None
    );
    assert_eq!(class_bit(3), 8);
    assert_eq!(class_bit(40), 0);
}

/// With the baseline 128 MiB binding an 8192 x 4096 map (512 MiB of texels) falls back to the
/// CPU; with the adapter's own limit the browser's GPU Worker requests (2 GiB on Chrome) it is
/// eligible, and a map past even that still is not.
#[test]
fn a_large_hdr_map_is_eligible_once_the_adapters_own_binding_limit_is_requested() {
    const BASELINE_BINDING: u64 = 128 * 1024 * 1024;
    const LIFTED_BINDING: u64 = 2_147_483_644;
    let texels = 8192 * 4096;
    let mut spec = spec_with("Diamond");
    spec.hdr_id = Some(7);
    assert_eq!(
        gpu_ineligibility_reason(&spec, BASELINE_BINDING, Some(texels), false),
        Some(REASON_HDR_TOO_BIG)
    );
    assert_eq!(
        gpu_ineligibility_reason(&spec, LIFTED_BINDING, Some(texels), false),
        None
    );
    assert_eq!(
        gpu_ineligibility_reason(&spec, LIFTED_BINDING, Some(16384 * 16384), false),
        Some(REASON_HDR_TOO_BIG)
    );
}

/// The adapter limits are part of the key, so lifting the requested binding limit starts every
/// browser on a fresh record (one re-test) while an adapter whose limit did not move keeps its
/// record.
#[test]
fn lifting_the_binding_limit_changes_the_adapter_key_once() {
    let baseline = AdapterFacts {
        max_storage_binding: 134_217_728,
        ..facts()
    };
    let lifted = AdapterFacts {
        max_storage_binding: 2_147_483_644,
        max_buffer: 4_294_967_296,
        ..facts()
    };
    assert_ne!(
        adapter_key(&baseline, "141", 1_u64),
        adapter_key(&lifted, "141", 1_u64)
    );
    assert_eq!(
        adapter_key(&lifted, "141", 1_u64),
        adapter_key(&lifted, "141", 1_u64)
    );
}

/// A diamond that scatters light inside (a hazy custom stone), named `name`.
fn scattering_spec(name: &str) -> SceneSpec {
    let mut hazy = GemMaterial::diamond();
    hazy.name = name.to_string();
    hazy.scattering_sigma_s = 1.0;
    assert!(hazy.scatters(), "a positive sigma scatters");
    let mut spec = spec_with(name);
    spec.material.custom_materials = vec![hazy];
    spec
}

#[test]
fn scene_needs_follow_scattering_hdr_and_frosted_facets() {
    let plain = scene_needs(&spec_with("Diamond")).expect("a catalogue stone resolves");
    assert_eq!(plain.class, ISOTROPIC_CLASS);
    assert!(plain.features.is_empty());

    let hazy = scene_needs(&scattering_spec("Hazy Diamond")).expect("a custom stone resolves");
    assert_eq!(hazy.class, ISOTROPIC_CLASS);
    assert_eq!(
        hazy.features.iter().collect::<Vec<_>>(),
        vec![KernelFeature::Scattering]
    );

    // Under an HDR map a scattering stone also needs the next-event kernel.
    let mut under_hdr = scattering_spec("Hazy Diamond");
    under_hdr.hdr_id = Some(3);
    let under_hdr = scene_needs(&under_hdr).expect("resolves");
    assert_eq!(
        under_hdr.features.iter().collect::<Vec<_>>(),
        vec![KernelFeature::Scattering, KernelFeature::ScatterNee]
    );
    // An HDR map alone needs nothing: the next-event kernel belongs to scattering.
    let mut clear_hdr = spec_with("Diamond");
    clear_hdr.hdr_id = Some(3);
    assert!(
        scene_needs(&clear_hdr)
            .expect("resolves")
            .features
            .is_empty()
    );

    let mut frosted = spec_with("Zircon");
    frosted.finishes = FinishSpec::PerFacet(vec![false, true]);
    let frosted = scene_needs(&frosted).expect("resolves");
    assert_eq!(frosted.class, UNIAXIAL_CLASS);
    assert!(frosted.features.contains(KernelFeature::Frosted));
    let mut polished = spec_with("Diamond");
    polished.finishes = FinishSpec::PerFacet(vec![false, false]);
    assert!(
        scene_needs(&polished)
            .expect("resolves")
            .features
            .is_empty()
    );

    assert_eq!(scene_needs(&spec_with("No Such Stone")), None);
}

/// A stone waits for the features it needs, per class; a feature of another class or of
/// another stone does not satisfy it.
#[test]
fn a_stone_that_needs_a_feature_waits_until_that_feature_is_compiled() {
    use pleochroma::renderer::engine::{EngineSpec, RenderEngine};

    let path = EngineSpec::default();
    let ray = EngineSpec {
        engine: RenderEngine::RayTraced,
        ..path
    };
    let bit = |class: u32, feature: KernelFeature| {
        class_bit(feature_unit(class, feature).expect("a material class has feature units"))
    };

    let hazy = scattering_spec("Hazy Diamond");
    assert_eq!(
        class_untested_reason(path, &hazy, 0),
        Some(REASON_SCATTER_NOT_COMPILED)
    );
    assert_eq!(
        class_untested_reason(path, &hazy, bit(ISOTROPIC_CLASS, KernelFeature::Scattering)),
        None
    );
    assert_eq!(
        class_untested_reason(path, &hazy, bit(UNIAXIAL_CLASS, KernelFeature::Scattering)),
        Some(REASON_SCATTER_NOT_COMPILED),
        "a uniaxial Scatter pipeline is another pipeline"
    );
    assert_eq!(
        class_untested_reason(path, &hazy, bit(ISOTROPIC_CLASS, KernelFeature::Frosted)),
        Some(REASON_SCATTER_NOT_COMPILED)
    );
    assert_eq!(class_untested_reason(ray, &hazy, 0), None);

    let mut under_hdr = scattering_spec("Hazy Diamond");
    under_hdr.hdr_id = Some(3);
    let scatter = bit(ISOTROPIC_CLASS, KernelFeature::Scattering);
    assert_eq!(
        class_untested_reason(path, &under_hdr, scatter),
        Some(REASON_SCATTER_NEE_NOT_COMPILED)
    );
    assert_eq!(
        class_untested_reason(
            path,
            &under_hdr,
            scatter | bit(ISOTROPIC_CLASS, KernelFeature::ScatterNee)
        ),
        None
    );

    let mut frosted = spec_with("Diamond");
    frosted.finishes = FinishSpec::PerFacet(vec![true]);
    assert_eq!(
        class_untested_reason(path, &frosted, scatter),
        Some(REASON_FROSTED_NOT_COMPILED)
    );
    assert_eq!(
        class_untested_reason(path, &frosted, bit(ISOTROPIC_CLASS, KernelFeature::Frosted)),
        None
    );

    // A plain stone never waits for a feature, and the class base set still comes first.
    assert_eq!(class_untested_reason(path, &spec_with("Diamond"), 0), None);
    let mut uniaxial_frosted = spec_with("Zircon");
    uniaxial_frosted.finishes = FinishSpec::PerFacet(vec![true]);
    assert_eq!(
        class_untested_reason(
            path,
            &uniaxial_frosted,
            bit(UNIAXIAL_CLASS, KernelFeature::Frosted)
        ),
        Some(REASON_CLASS_NOT_TESTED)
    );
    assert_eq!(
        class_untested_reason(
            path,
            &uniaxial_frosted,
            class_bit(UNIAXIAL_CLASS) | bit(UNIAXIAL_CLASS, KernelFeature::Frosted)
        ),
        None
    );
}

/// Every compile unit has an id of its own and its own bit in the pool's mask.
#[test]
fn compile_units_have_distinct_ids_and_bits_that_round_trip() {
    let mut units = vec![
        ISOTROPIC_CLASS,
        UNIAXIAL_CLASS,
        BIAXIAL_CLASS,
        WHITTED_CLASS,
    ];
    for class in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        for feature in KernelFeature::ALL {
            let unit = feature_unit(class, feature).expect("a material class has feature units");
            assert_eq!(unit_class(unit), class, "{feature:?}");
            assert_eq!(unit_feature(unit), Some(feature), "class {class}");
            assert_eq!(
                kernels_for_unit(unit),
                feature.kernels(),
                "{feature:?} compiles its own kernels"
            );
            units.push(unit);
        }
        assert_eq!(unit_feature(class), None);
        assert_eq!(unit_class(class), class);
        assert_eq!(kernels_for_unit(class), kernels_for_class(class));
        // A class value that carries feature bits names the same unit as its kind.
        assert_eq!(
            feature_unit(browser_pipeline_class(class), KernelFeature::Scattering),
            feature_unit(class, KernelFeature::Scattering)
        );
    }
    let mut bits: Vec<u32> = units.iter().map(|unit| class_bit(*unit)).collect();
    assert!(
        bits.iter().all(|bit| bit.is_power_of_two()),
        "every unit fits the mask"
    );
    bits.sort_unstable();
    bits.dedup();
    assert_eq!(bits.len(), units.len(), "no two units share a bit");
    // Ray-Traced has no features, and ids that name nothing have none either.
    assert_eq!(feature_unit(WHITTED_CLASS, KernelFeature::Scattering), None);
    assert_eq!(feature_unit(0, KernelFeature::Frosted), None);
    assert_eq!(unit_feature(8), None);
    assert_eq!(unit_feature(99), None);
}

#[test]
fn feature_reasons_name_their_feature_and_nothing_else() {
    for feature in KernelFeature::ALL {
        assert_eq!(feature_for_reason(feature.reason()), Some(feature));
    }
    for other in [
        REASON_CLASS_NOT_TESTED,
        REASON_ANISOTROPIC,
        REASON_ZONED,
        REASON_UNRESOLVED,
        "",
    ] {
        assert_eq!(feature_for_reason(other), None, "{other}");
    }
    let reasons: std::collections::BTreeSet<&str> =
        KernelFeature::ALL.iter().map(|f| f.reason()).collect();
    assert_eq!(reasons.len(), KernelFeature::ALL.len());
    // Sentence fragments in the style of the existing reasons: lower case, no final period.
    for reason in reasons {
        assert!(reason.starts_with("this stone"), "{reason}");
        assert!(!reason.ends_with('.'), "{reason}");
    }
}

/// A feature has a record and a crash marker of its own: a failed `Scatter` compile is stored
/// against the scattering feature, not against the class or the other features.
#[test]
fn feature_keys_are_separate_from_class_and_from_each_other() {
    let mut keys = Vec::new();
    for class in [ISOTROPIC_CLASS, UNIAXIAL_CLASS, BIAXIAL_CLASS] {
        keys.push(unit_adapter_key(&facts(), "141", 7_u64, class));
        for feature in KernelFeature::ALL {
            let unit = feature_unit(class, feature).expect("feature unit");
            keys.push(unit_adapter_key(&facts(), "141", 7_u64, unit));
        }
    }
    keys.push(whitted_adapter_key(&facts(), "141", "7"));
    let mut distinct = keys.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), keys.len(), "every unit has its own key");
    for key in &keys {
        assert!(key.starts_with("pleochroma.admission."), "{key}");
    }
    // A class unit's key is the class key; the feature key is the class key's text plus a tag.
    assert_eq!(
        unit_adapter_key(&facts(), "141", 7_u64, UNIAXIAL_CLASS),
        class_adapter_key(&facts(), "141", 7_u64, UNIAXIAL_CLASS)
    );
    let scatter = feature_unit(ISOTROPIC_CLASS, KernelFeature::Scattering).expect("feature unit");
    assert_eq!(
        unit_adapter_key(&facts(), "141", 7_u64, scatter),
        adapter_key(
            &facts(),
            "141",
            format!("7:c1:p{BROWSER_PIPELINE_VARIANT}:fscatter")
        )
    );
    // The browser major version is part of every key, so a Chrome update re-tests them all.
    assert_ne!(
        unit_adapter_key(&facts(), "141", 7_u64, scatter),
        unit_adapter_key(&facts(), "142", 7_u64, scatter)
    );
}

/// The stored shape changed (the class record no longer covers `Scatter`), so the records of
/// version 1 are not read: a user whose `Scatter` compile failed retests the plain stones.
#[test]
fn the_record_variant_was_raised_for_the_split() {
    const { assert!(BROWSER_PIPELINE_VARIANT >= 2) };
    let key = class_adapter_key(&facts(), "141", 7_u64, ISOTROPIC_CLASS);
    assert_ne!(key, adapter_key(&facts(), "141", "7:c1:p1"));
}

#[test]
fn the_soft_compile_limit_covers_the_measured_slow_kernels_and_stays_below_the_hard_one() {
    // The slowest single kernel measured on Chrome 156 / RDNA2 was Scatter at 11 s.
    const { assert!(COMPILE_SOFT_MS >= 11_000) };
    assert_eq!(COMPILE_SOFT_MS, 15_000);
    const { assert!(COMPILE_SOFT_MS < COMPILE_HARD_MS) };
    const { assert!(COMPILE_HARD_MS < STAGE_DEADLINE_MS) };
}

#[test]
fn the_worker_ledger_remembers_kernels_per_kind() {
    let mut ledger = CompiledKernels::default();
    assert!(!ledger.contains(ISOTROPIC_CLASS, "Tail"));
    ledger.record(ISOTROPIC_CLASS, "Tail");
    ledger.record(browser_pipeline_class(UNIAXIAL_CLASS), "Scatter");
    assert!(ledger.contains(ISOTROPIC_CLASS, "Tail"));
    // A class value with feature bits and a feature unit id both reach their kind's entry.
    assert!(ledger.contains(browser_pipeline_class(ISOTROPIC_CLASS), "Tail"));
    assert!(ledger.contains(UNIAXIAL_CLASS, "Scatter"));
    assert!(!ledger.contains(ISOTROPIC_CLASS, "Scatter"));
    assert!(!ledger.contains(BIAXIAL_CLASS, "Tail"));
    assert_eq!(
        ledger.missing(ISOTROPIC_CLASS, ["Tail", "Scatter", "Finalize"]),
        vec!["Scatter", "Finalize"]
    );
    assert!(ledger.missing(ISOTROPIC_CLASS, ["Tail"]).is_empty());
    ledger.clear();
    assert_eq!(ledger, CompiledKernels::default());
}

/// Compiling the base set of a class leaves a scattering stone short of exactly the scattering
/// kernels: the Worker's ledger and the stone's needs agree on what is missing.
#[test]
fn a_base_set_compile_leaves_a_scattering_stone_missing_only_its_feature_kernels() {
    let mut ledger = CompiledKernels::default();
    for kernel in kernels_for_class(ISOTROPIC_CLASS) {
        ledger.record(ISOTROPIC_CLASS, kernel);
    }
    let needs = scene_needs(&scattering_spec("Hazy Diamond")).expect("resolves");
    let wanted: Vec<&str> = kernels_for_class(needs.class)
        .iter()
        .copied()
        .chain(
            needs
                .features
                .iter()
                .flat_map(|feature| feature.kernels().iter().copied()),
        )
        .collect();
    assert_eq!(
        ledger.missing(needs.class, wanted.iter().copied()),
        vec!["Scatter"]
    );
    for kernel in KernelFeature::Scattering.kernels() {
        ledger.record(needs.class, kernel);
    }
    assert!(
        ledger
            .missing(needs.class, wanted.iter().copied())
            .is_empty()
    );
}

#[test]
fn feature_compile_texts_say_which_part_compiles() {
    assert_eq!(
        feature_compile_label(KernelFeature::Scattering, ISOTROPIC_CLASS, 0, 1),
        "Compiling the scattering kernels"
    );
    assert_eq!(
        feature_compile_label(KernelFeature::Frosted, UNIAXIAL_CLASS, 0, 1),
        "Compiling the frosted-facet kernels for uniaxial stones"
    );
    // A feature with several kernels shows the count.
    assert_eq!(
        feature_compile_label(KernelFeature::ScatterNee, BIAXIAL_CLASS, 1, 2),
        "Compiling the HDR scattering kernels (1/2) for biaxial stones"
    );
    assert_eq!(
        feature_passed_text(KernelFeature::Scattering, ISOTROPIC_CLASS, 9_500.0),
        "GPU scattering kernels compiled (total 9.5 s)"
    );
    assert_eq!(
        feature_passed_text(KernelFeature::Frosted, UNIAXIAL_CLASS, 1_300.0),
        "GPU frosted-facet kernels compiled for uniaxial stones (total 1.3 s)"
    );
}
