use super::{
    test_support::{
        curves, hdr_scene, panned_still_job, still_job, turned_hdr_still_job, video_job,
    },
    *,
};
#[cfg(beam_tests)]
use pleochroma::renderer::accel_beam::BeamSettings;
use pleochroma::renderer::adaptive::AdaptiveSettings;

fn message(job: &RenderJobFile) -> String {
    job.validate().unwrap_err().to_string()
}

fn video_mut(job: &mut RenderJobFile) -> &mut TiltVideoJob {
    match &mut job.kind {
        JobKind::TiltVideo(video) => video,
        JobKind::Still(_) => unreachable!("the test job is a video"),
    }
}

#[test]
fn valid_jobs_pass() {
    assert_eq!(still_job().validate(), Ok(()));
    assert_eq!(video_job().validate(), Ok(()));
}

#[test]
fn refuses_a_wrong_format() {
    let mut job = still_job();
    job.format = 2;
    assert!(message(&job).contains("format 2"));
}

#[test]
fn refuses_a_bad_token() {
    for token in [
        "",
        "abc",
        "9F2C4A7D1E0B48C3A65D2F71C8E4B903",
        "9f2c4a7d1e0b48c3a65d2f71c8e4b90g",
    ] {
        let mut job = still_job();
        job.token = token.to_string();
        assert_eq!(
            message(&job),
            "The job token must be 32 lowercase hexadecimal characters."
        );
    }
}

#[test]
fn refuses_an_empty_output() {
    let mut job = still_job();
    job.output = "  ".to_string();
    assert_eq!(message(&job), "The job has no output location.");
}

#[test]
fn refuses_a_bad_size() {
    for (width, height) in [(15, 48), (64, 15), (8193, 48), (64, 8193)] {
        let mut job = still_job();
        job.scene.width = width;
        job.scene.height = height;
        assert_eq!(
            message(&job),
            "The picture size must be between 16 and 8192 pixels."
        );
    }
    let mut job = still_job();
    job.scene.width = 16;
    job.scene.height = 8192;
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn refuses_bad_samples() {
    for spp in [0, MAX_SPP + 1] {
        let mut job = still_job();
        job.kind = JobKind::Still(StillJob {
            samples_per_pixel: spp,
            color_space: JobColorSpace::Srgb,
            preset_label: String::new(),
            accel: RenderAccel::default(),
        });
        assert_eq!(
            message(&job),
            "The samples per pixel must be between 1 and 32768."
        );
        let mut video = video_job();
        video_mut(&mut video).samples_per_pixel = spp;
        assert!(message(&video).contains("samples per pixel"));
    }
}

#[test]
fn refuses_bad_bounces() {
    for bounces in [0, MAX_BOUNCES + 1] {
        let mut job = still_job();
        job.scene.max_bounces = bounces;
        assert_eq!(message(&job), "The bounce limit must be between 1 and 128.");
    }
}

#[test]
fn refuses_bad_video_numbers() {
    let mut job = video_job();
    video_mut(&mut job).fps = 0;
    assert!(message(&job).contains("frame rate"));
    video_mut(&mut job).fps = MAX_FPS + 1;
    assert!(message(&job).contains("frame rate"));

    let mut job = video_job();
    video_mut(&mut job).total_frames = 0;
    assert!(message(&job).contains("between 1 and 18001 frames"));
    video_mut(&mut job).total_frames = MAX_FRAMES + 1;
    assert!(message(&job).contains("between 1 and 18001 frames"));

    for step in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut job = video_job();
        video_mut(&mut job).step_deg = step;
        assert_eq!(message(&job), "The tilt step must be a number above zero.");
    }
    let mut job = video_job();
    video_mut(&mut job).end_deg = f64::NAN;
    assert!(message(&job).contains("start and end angles"));
}

#[test]
fn refuses_short_curves_and_missing_curves() {
    let mut job = video_job();
    let mut short = curves();
    short.windowing.truncate(CURVE_POINTS - 1);
    video_mut(&mut job).curves = Some(short);
    assert!(message(&job).contains("at least 181 values"));

    let mut job = video_job();
    video_mut(&mut job).curves = None;
    assert!(message(&job).contains("needs the metric curves"));

    // No overlay and no curves is fine.
    video_mut(&mut job).overlay = OverlaySelection::default();
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn refuses_hdr_mismatches() {
    let mut job = video_job();
    job.hdr = None;
    assert!(message(&job).contains("does not name the map file"));

    let mut job = still_job();
    job.hdr = video_job().hdr;
    assert!(message(&job).contains("not lit by an HDR map"));

    let mut job = video_job();
    if let Some(hdr) = job.hdr.as_mut() {
        hdr.sha256_hex = "00".repeat(32);
    }
    assert!(message(&job).contains("does not match"));

    let (scene, source) = hdr_scene();
    let mut job = still_job();
    job.scene = scene;
    job.hdr = Some(source);
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn kind_words_and_frame_counts() {
    assert_eq!(still_job().kind_word(), "still");
    assert_eq!(still_job().frames_total(), 1);
    assert_eq!(video_job().kind_word(), "tilt_video");
    assert_eq!(video_job().frames_total(), 181);
}

#[test]
fn resolves_relative_paths_against_the_base() {
    let job = video_job();
    let base = Path::new("base");
    assert_eq!(job.output_path(base), base.join("frames"));
    assert_eq!(
        job.hdr_path(base),
        Some(base.join("assets").join("studio.hdr"))
    );
    assert_eq!(still_job().hdr_path(base), None);
    let absolute = std::env::temp_dir().join("x.png");
    let text = absolute.to_string_lossy().into_owned();
    assert_eq!(job.resolve(&text, base), absolute);
}

#[test]
fn cli_words_round_trip() {
    for choice in [
        ComputeChoice::Local,
        ComputeChoice::Remote,
        ComputeChoice::Both,
    ] {
        assert_eq!(
            ComputeChoice::from_cli_word(choice.cli_word()),
            Some(choice)
        );
    }
    for choice in [TransferChoice::FullData, TransferChoice::FinalPicture] {
        assert_eq!(
            TransferChoice::from_cli_word(choice.cli_word()),
            Some(choice)
        );
    }
    for engines in [LocalEngines::Cpu, LocalEngines::CpuGpu, LocalEngines::Gpu] {
        assert_eq!(
            LocalEngines::from_cli_word(engines.cli_word()),
            Some(engines)
        );
    }
    assert_eq!(
        LocalEngines::from_cli_word("CPUGPU"),
        Some(LocalEngines::CpuGpu)
    );
    assert_eq!(
        LocalEngines::from_cli_word("cpu-gpu"),
        Some(LocalEngines::CpuGpu)
    );
    assert_eq!(LocalEngines::from_cli_word("tpu"), None);
    assert_eq!(ComputeChoice::from_cli_word("all"), None);
    assert_eq!(TransferChoice::from_cli_word("raw"), None);
    assert_eq!(LocalEngines::default(), LocalEngines::CpuGpu);
}

#[test]
fn labels_and_overlay_count() {
    assert_eq!(JobColorSpace::Srgb.label(), "sRGB");
    assert_eq!(JobColorSpace::DisplayP3.label(), "Display P3");
    assert_eq!(JobColorSpace::Rec2020.label(), "Rec.2020");
    assert_eq!(OverlaySelection::default().count(), 0);
    let all = OverlaySelection {
        brilliance: true,
        windowing: true,
        extinction: true,
        tilt_brilliance: true,
        angle: true,
    };
    assert_eq!(all.count(), 5);
}

#[test]
fn tokens_have_the_right_shape_and_differ() {
    let first = new_job_token();
    let second = new_job_token();
    assert!(is_token(&first), "{first}");
    assert!(is_token(&second), "{second}");
    assert_ne!(first, second);
}

#[test]
fn diagnostic_options_roundtrip_and_old_jobs_default_to_clean_output() {
    let mut job = super::test_support::still_job();
    let clean = serde_json::to_value(&job).unwrap();
    assert!(clean.get("optical_diagnostics").is_none());
    let old: RenderJobFile = serde_json::from_value(clean).unwrap();
    assert!(old.optical_diagnostics.is_none());
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: 1,
        layers: [true, false, true, true],
        opacity: 0.45,
        map_mode: false,
        high_contrast: true,
        scintillation: false,
    });
    let copy: RenderJobFile = serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
    assert_eq!(copy.optical_diagnostics, job.optical_diagnostics);
    job.optical_diagnostics.as_mut().unwrap().model_version = 2;
    assert!(job.validate().is_err());
}

#[test]
fn scintillation_defaults_off_is_written_only_when_on_and_round_trips() {
    let mut job = super::test_support::still_job();
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: SUPPORTED_OPTICAL_MODEL_VERSION,
        layers: [true, false, false, false],
        opacity: 0.45,
        map_mode: false,
        high_contrast: false,
        scintillation: false,
    });
    // Off: no key, so the file keeps its existing bytes.
    let value = serde_json::to_value(&job).unwrap();
    let options = value.get("optical_diagnostics").expect("diagnostics");
    assert!(options.get("scintillation").is_none(), "{options}");
    // A file that predates the key reads as off.
    let old: RenderJobFile = serde_json::from_value(value).unwrap();
    assert_eq!(old.optical_diagnostics, job.optical_diagnostics);
    assert!(!old.optical_diagnostics.unwrap().scintillation);
    // On: written, read back, still valid at the same model version.
    job.optical_diagnostics.as_mut().unwrap().scintillation = true;
    assert!(job.validate().is_ok());
    let value = serde_json::to_value(&job).unwrap();
    assert_eq!(
        value["optical_diagnostics"]["scintillation"],
        serde_json::Value::Bool(true)
    );
    let copy: RenderJobFile = serde_json::from_value(value).unwrap();
    assert_eq!(copy.optical_diagnostics, job.optical_diagnostics);
}

#[test]
fn diagnostics_serialize_after_the_format_header_and_use_the_supported_model_version() {
    let mut job = super::test_support::still_job();
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: SUPPORTED_OPTICAL_MODEL_VERSION,
        layers: [true, false, false, false],
        opacity: 0.45,
        map_mode: false,
        high_contrast: false,
        scintillation: false,
    });
    assert!(job.validate().is_ok());
    let text = serde_json::to_string(&job).unwrap();
    let format_at = text.find("\"format\"").expect("format key");
    let diagnostics_at = text
        .find("\"optical_diagnostics\"")
        .expect("diagnostics key");
    assert!(
        format_at < diagnostics_at,
        "the format header must come before the optional diagnostics: {text}"
    );
}

/// A job file without a `frosted_planes` key reads as unfrosted and keeps its format version.
/// A job that frosts planes round-trips them through the codec.
#[test]
fn old_jobs_read_unfrosted_and_frosted_planes_round_trip() {
    let job = super::test_support::still_job();
    assert_eq!(job.scene.frosted_planes, Vec::<u32>::new());
    let mut value = serde_json::to_value(&job).unwrap();
    let scene = value
        .get_mut("scene")
        .and_then(serde_json::Value::as_object_mut)
        .expect("the job has a scene object");
    assert_eq!(
        scene.remove("frosted_planes"),
        Some(serde_json::json!([])),
        "the scene writes the (empty) list"
    );
    let old: RenderJobFile = serde_json::from_value(value).unwrap();
    assert!(
        old.scene.frosted_planes.is_empty(),
        "an old job is unfrosted"
    );
    assert_eq!(old, job, "and is otherwise the same job");
    assert_eq!(
        old.format, 1,
        "legacy Path-Traced jobs keep their original format"
    );

    let mut frosted = super::test_support::still_job();
    frosted.scene.frosted_planes = vec![3, 4, 30];
    let text = crate::codec::to_text(&frosted).unwrap();
    let back = crate::codec::from_text(&text).unwrap();
    assert_eq!(back.scene.frosted_planes, vec![3, 4, 30]);
    assert_eq!(back, frosted);
}

/// A color variant on the scene's material is written into the job file with its per-axis
/// tensor and unit. Jobs that differ only in the variant are different files.
#[test]
fn a_color_variant_material_is_written_to_the_job_file_and_reads_back() {
    use pleochroma::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{AbsorptionUnit, GemMaterial},
    };

    let tensor = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::new(550.0, 40.0, 0.35)],
        vec![AbsorptionBand::energy(430.0, 1500.5, 0.12)],
    );
    let mut plain = still_job();
    plain.scene.material = GemMaterial::sapphire();
    let mut recolored = plain.clone();
    recolored.scene.material =
        GemMaterial::sapphire().with_color_variant_tensor(tensor.clone(), AbsorptionUnit::PerMm);
    assert_ne!(plain, recolored);

    let plain_text = crate::codec::to_text(&plain).unwrap();
    let recolored_text = crate::codec::to_text(&recolored).unwrap();
    assert_ne!(plain_text, recolored_text, "the variant is in the file");

    let back = crate::codec::from_text(&recolored_text).unwrap();
    assert_eq!(back.scene.material.name, "Sapphire");
    assert_eq!(back.scene.material.absorption, tensor);
    assert_eq!(back.scene.material.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(back.validate(), Ok(()));
}

/// A job's JSON without the `scene_backdrop_model` key, holding the given card level.
fn legacy_text(card: f32) -> String {
    let mut value = serde_json::to_value(still_job()).unwrap();
    let object = value.as_object_mut().unwrap();
    assert_eq!(
        object.remove("scene_backdrop_model"),
        Some(serde_json::json!(BACKDROP_MODEL_TRAY)),
        "writers write the tray marker"
    );
    value["scene"]["backdrop"] = serde_json::json!(card);
    value.to_string()
}

#[test]
fn a_legacy_card_level_converts_to_the_tray_of_the_files_own_preset_and_pose() {
    use pleochroma::render_setup::Backdrop;
    let reference = still_job().scene;
    let tray = |choice: Backdrop| {
        choice.resolve(
            reference.lighting_preset,
            reference.light_yaw,
            reference.light_pitch,
        )
    };
    let grey = crate::codec::from_text(&legacy_text(0.23)).unwrap();
    assert_eq!(grey.scene_backdrop_model, BACKDROP_MODEL_TRAY);
    assert_eq!(
        grey.scene.backdrop.to_bits(),
        tray(Backdrop::Grey).to_bits()
    );
    assert!(grey.scene.backdrop > 0.0);
    let white = crate::codec::from_text(&legacy_text(8.0)).unwrap();
    assert_eq!(
        white.scene.backdrop.to_bits(),
        tray(Backdrop::White).to_bits()
    );
    assert!(white.scene.backdrop < 8.0, "no longer an 8.0 emitter");
    let as_lit = crate::codec::from_text(&legacy_text(0.0)).unwrap();
    assert_eq!(as_lit.scene.backdrop.to_bits(), 0.0_f32.to_bits());
    assert_eq!(as_lit.scene_backdrop_model, BACKDROP_MODEL_TRAY);
    let other = crate::codec::from_text(&legacy_text(0.5)).unwrap();
    assert_eq!(other.scene.backdrop.to_bits(), 0.5_f32.to_bits());
}

#[test]
fn the_conversion_follows_the_files_own_lighting() {
    use pleochroma::optics::raytracer::LightingPreset;
    let mut value: serde_json::Value = serde_json::from_str(&legacy_text(8.0)).unwrap();
    value["scene"]["lighting_preset"] = serde_json::to_value(LightingPreset::WhiteTray).unwrap();
    let job = crate::codec::from_text(&value.to_string()).unwrap();
    let expected = pleochroma::render_setup::Backdrop::White.resolve(
        LightingPreset::WhiteTray,
        job.scene.light_yaw,
        job.scene.light_pitch,
    );
    assert_eq!(job.scene.backdrop.to_bits(), expected.to_bits());
    let daylight = crate::codec::from_text(&legacy_text(8.0)).unwrap();
    assert_ne!(
        daylight.scene.backdrop.to_bits(),
        job.scene.backdrop.to_bits(),
        "a different preset resolves a different tray"
    );
}

#[test]
fn a_job_with_the_tray_marker_is_not_converted_again() {
    let mut job = still_job();
    job.scene.backdrop = 0.23;
    let back = crate::codec::from_text(&crate::codec::to_text(&job).unwrap()).unwrap();
    assert_eq!(back.scene.backdrop.to_bits(), 0.23_f32.to_bits());
    assert_eq!(back, job);
    let mut again = back.clone();
    again.upgrade_backdrop_model();
    assert_eq!(again, back, "the upgrade is idempotent");
}

#[test]
fn an_unknown_backdrop_model_is_refused() {
    let mut job = still_job();
    job.scene_backdrop_model = BACKDROP_MODEL_TRAY + 1;
    assert!(message(&job).contains("backdrop model 2"));
    job.scene_backdrop_model = BACKDROP_MODEL_LEGACY_CARD;
    assert_eq!(job.validate(), Ok(()), "the legacy marker is accepted");
}

fn adaptive_on(spp: u32) -> RenderAccel {
    RenderAccel {
        adaptive: Some(AdaptiveSettings::provisional(spp)),
        path_cache: None,
        beam: None,
    }
}

fn still_mut(job: &mut RenderJobFile) -> &mut StillJob {
    match &mut job.kind {
        JobKind::Still(still) => still,
        JobKind::TiltVideo(_) => unreachable!("the test job is a still"),
    }
}

/// A job without an `accel` key reads with acceleration off, and a job with everything off
/// writes no key.
#[test]
fn old_jobs_read_with_acceleration_off_and_an_off_job_writes_no_key() {
    for job in [still_job(), video_job()] {
        let mut value = serde_json::to_value(&job).unwrap();
        let kind = value.get_mut("kind").unwrap().as_object_mut().unwrap();
        assert_eq!(kind.remove("accel"), None, "an off job writes no key");
        assert_eq!(kind.remove("keep_path_cache"), None);
        let old: RenderJobFile = serde_json::from_value(value).unwrap();
        assert_eq!(old, job);
        let text = crate::codec::to_text(&job).unwrap();
        assert!(!text.contains("accel"), "{text}");
        assert!(!text.contains("keep_path_cache"), "{text}");
    }
}

#[test]
fn a_still_with_adaptive_sampling_round_trips_through_the_codec() {
    let mut job = still_job();
    let mut accel = adaptive_on(64);
    if let Some(adaptive) = accel.adaptive.as_mut() {
        adaptive.target_noise = 0.02;
    }
    still_mut(&mut job).accel = accel.clone();
    let text = crate::codec::to_text(&job).unwrap();
    assert!(text.contains("accel"), "{text}");
    let back = crate::codec::from_text(&text).unwrap();
    assert_eq!(back, job);
    let JobKind::Still(still) = &back.kind else {
        unreachable!("a still stays a still")
    };
    assert_eq!(still.accel, accel);
}

#[test]
fn a_video_with_adaptive_sampling_and_a_kept_cache_round_trips_through_the_codec() {
    let mut job = video_job();
    let video = video_mut(&mut job);
    video.accel = adaptive_on(video.samples_per_pixel);
    video.keep_path_cache = true;
    let text = crate::codec::to_text(&job).unwrap();
    assert!(text.contains("keep_path_cache"), "{text}");
    let back = crate::codec::from_text(&text).unwrap();
    assert_eq!(back, job);
}

/// A hand-edited job with beam settings a worker refuses is refused when it loads.
#[cfg(beam_tests)]
#[test]
fn beam_settings_outside_their_range_are_refused() {
    let mut job = still_job();
    still_mut(&mut job).accel.beam = Some(BeamSettings::default());
    assert_eq!(job.validate(), Ok(()));
    still_mut(&mut job).accel.beam = Some(BeamSettings {
        spectral_nodes: 12,
        ..BeamSettings::default()
    });
    assert!(
        message(&job).starts_with("Beam tracing: "),
        "{}",
        message(&job)
    );
    let mut video = video_job();
    video_mut(&mut video).accel.beam = Some(BeamSettings {
        spectral_nodes: 12,
        ..BeamSettings::default()
    });
    assert!(message(&video).starts_with("Beam tracing: "));
}

#[test]
fn adaptive_settings_are_checked_against_the_jobs_own_sample_count() {
    let mut job = still_job();
    still_mut(&mut job).accel = adaptive_on(64);
    assert_eq!(job.validate(), Ok(()));
    // The stored cap is replaced by the job's samples, so a stale cap is harmless.
    if let Some(adaptive) = still_mut(&mut job).accel.adaptive.as_mut() {
        adaptive.max_spp = 9999;
    }
    assert_eq!(job.validate(), Ok(()));
    // A target outside its range is refused with a sentence.
    if let Some(adaptive) = still_mut(&mut job).accel.adaptive.as_mut() {
        adaptive.target_noise = 3.0;
    }
    assert!(
        message(&job).starts_with("Adaptive sampling: "),
        "{}",
        message(&job)
    );
    // A minimum above the job's samples is refused.
    let mut job = still_job();
    still_mut(&mut job).accel = adaptive_on(512);
    assert!(
        message(&job).starts_with("Adaptive sampling: "),
        "{}",
        message(&job)
    );
    let mut video = video_job();
    let spp = video_mut(&mut video).samples_per_pixel;
    video_mut(&mut video).accel = adaptive_on(spp);
    assert_eq!(video.validate(), Ok(()));
    video_mut(&mut video).accel = adaptive_on(512);
    assert!(message(&video).starts_with("Adaptive sampling: "));
}

#[test]
fn adaptive_sampling_is_refused_in_a_ray_traced_job() {
    use pleochroma::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    let mut job = still_job();
    job.engine = EngineSpec {
        engine: RenderEngine::RayTraced,
        ray: RayOptions {
            aa_taps: 4,
            fire: false,
        },
    };
    job.scene.engine = job.engine;
    job.format = 2;
    let still = still_mut(&mut job);
    still.samples_per_pixel = 4;
    assert_eq!(job.validate(), Ok(()));
    still_mut(&mut job).accel = adaptive_on(4);
    assert_eq!(
        message(&job),
        "Adaptive sampling requires Path-Traced rendering."
    );
}

/// An `accel` record with none of its keys reads with acceleration off, since every
/// `RenderAccel` field defaults.
#[test]
fn an_accel_record_without_its_inner_keys_reads_as_off() {
    let mut value = serde_json::to_value(still_job()).unwrap();
    value["kind"]["accel"] = serde_json::json!({});
    let back: RenderJobFile = serde_json::from_value(value).unwrap();
    assert_eq!(back, still_job());
}

/// A job carries the scene's render effects (inclusions, facet surfaces, the faceting
/// machine). They survive the codec, a job without the key reads as no effects, and a record
/// no sender could produce is refused.
#[test]
fn the_scene_effects_round_trip_old_jobs_read_none_and_bad_records_are_refused() {
    use pleochroma::optics::{
        inclusions::spec::InclusionSpec,
        raytracer::surface::{FacetSurface, PolishPreset},
    };

    let job = super::test_support::still_job();
    assert!(job.scene.effects.is_none());
    let mut value = serde_json::to_value(&job).unwrap();
    let scene = value
        .get_mut("scene")
        .and_then(serde_json::Value::as_object_mut)
        .expect("the job has a scene object");
    assert!(
        scene.remove("effects").is_none(),
        "a scene without effects writes no record to the job file"
    );
    let old: RenderJobFile = serde_json::from_value(value).unwrap();
    assert!(old.scene.effects.is_none(), "an old job has no effects");
    assert_eq!(old, job, "and is otherwise the same job");

    let mut busy = super::test_support::still_job();
    busy.scene.effects.inclusions = Some(InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 0.35,
        turn_deg: None,
        sheen: None,
        color: None,
    });
    busy.scene.effects.facet_surfaces =
        vec![FacetSurface::from_preset(PolishPreset::Fine, 0.3).packed(); 4];
    busy.scene.effects.machine.arm_offset_deg = 15.0;
    let text = crate::codec::to_text(&busy).unwrap();
    assert!(text.contains("effects"), "a busy scene writes its record");
    let back = crate::codec::from_text(&text).unwrap();
    assert_eq!(back.scene.effects, busy.scene.effects);
    assert_eq!(back, busy);

    let mut bad = busy;
    bad.scene.effects.inclusions.as_mut().unwrap().density = f32::NAN;
    assert!(
        message(&bad).contains("render effects"),
        "{}",
        message(&bad)
    );
}

/// Format 3 prevents an old reader from silently losing the shadow option.
#[test]
fn a_shadowed_job_requires_format_three_and_a_supported_scene() {
    let mut job = still_job();
    job.scene.stone_shadow = true;
    job.scene.backdrop = 1.0;
    job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    assert_eq!(format_for(&job), 3);
    assert!(job.validate().is_err());
    job.format = format_for(&job);
    assert_eq!(job.validate(), Ok(()));
    job.scene.backdrop = 0.0;
    assert!(message(&job).contains("Stone shadow needs a studio light with a backdrop."));
    // `video_job` is lit by an HDR map, which the studio-light rule would refuse first; give the
    // video the studio scene so the tilt-video rule is the one that speaks.
    let mut video = video_job();
    video.scene = still_job().scene;
    video.hdr = None;
    video.scene.stone_shadow = true;
    video.scene.backdrop = 1.0;
    video.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    video.format = format_for(&video);
    assert!(message(&video).contains("Tilt videos do not show the stone shadow yet."));
}

/// The HDR and ASET lights have no tray to shadow, so a shadowed job on either is refused.
#[test]
fn a_shadowed_job_is_refused_under_an_hdr_map_or_the_aset_light() {
    let (scene, source) = hdr_scene();
    let mut job = still_job();
    job.scene = scene;
    job.hdr = Some(source);
    job.scene.stone_shadow = true;
    job.scene.backdrop = 1.0;
    job.format = format_for(&job);
    assert!(message(&job).contains("Stone shadow needs a studio light with a backdrop."));

    let mut aset = still_job();
    aset.scene.stone_shadow = true;
    aset.scene.backdrop = 1.0;
    aset.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::Aset;
    aset.format = format_for(&aset);
    assert!(message(&aset).contains("Stone shadow needs a studio light with a backdrop."));
}

/// Format 4 prevents an old reader from silently losing the tray reflection; it wins over the
/// shadow's format 3 when both are set.
#[test]
fn a_reflective_job_requires_format_four_and_a_supported_scene() {
    let mut job = still_job();
    job.scene.tray_reflection = 0.04;
    job.scene.backdrop = 1.0;
    job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    assert_eq!(format_for(&job), 4);
    assert!(job.validate().is_err());
    job.format = format_for(&job);
    assert_eq!(job.validate(), Ok(()));
    job.scene.stone_shadow = true;
    assert_eq!(format_for(&job), 4, "the reflection's format wins");
    job.scene.stone_shadow = false;
    job.scene.backdrop = 0.0;
    assert!(message(&job).contains("Tray reflection needs a studio light with a backdrop."));
    job.scene.backdrop = 1.0;
    job.scene.tray_reflection = 1.5;
    assert!(message(&job).contains("Tray reflection must be between 0 and 100 %."));
    job.scene.tray_reflection = f32::NAN;
    job.format = format_for(&job);
    assert!(message(&job).contains("Tray reflection must be between 0 and 100 %."));
    // As for the shadow, the video gets the studio scene so the tilt-video rule is the one that
    // speaks.
    let mut video = video_job();
    video.scene = still_job().scene;
    video.hdr = None;
    video.scene.tray_reflection = 0.04;
    video.scene.backdrop = 1.0;
    video.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    video.format = format_for(&video);
    assert!(message(&video).contains("Tilt videos do not show the tray reflection yet."));
}

/// A hand-edited format 4 file with a negative reflectance is told its reflectance is out of
/// range, not that its format does not match (`format_for` reads a negative reflectance as none).
#[test]
fn a_hand_edited_negative_reflectance_is_refused_for_the_reflectance() {
    let mut job = still_job();
    job.scene.tray_reflection = 0.04;
    job.scene.backdrop = 1.0;
    job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    job.format = format_for(&job);
    assert_eq!(job.format, 4);
    assert_eq!(job.validate(), Ok(()));
    job.scene.tray_reflection = -0.5;
    assert_eq!(job.format, 4, "the file still says format 4");
    assert!(message(&job).contains("Tray reflection must be between 0 and 100 %."));
}

/// A job without the reflection writes no `tray_reflection` key.
#[test]
fn a_reflection_free_job_writes_no_tray_reflection_key() {
    let job = still_job();
    assert!(
        !crate::codec::to_text(&job)
            .unwrap()
            .contains("tray_reflection")
    );
    let mut reflective = still_job();
    reflective.scene.tray_reflection = 0.04;
    reflective.scene.backdrop = 1.0;
    reflective.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    reflective.format = format_for(&reflective);
    assert!(
        crate::codec::to_text(&reflective)
            .unwrap()
            .contains("tray_reflection")
    );
}

/// Zones need format 5 whatever else the job holds; it wins over the reflection's format 4.
#[test]
fn a_zoned_job_requires_format_five_and_usable_zones() {
    let mut job = still_job();
    assert_eq!(format_for(&job), 1, "no zones, no format 5");
    job.zoning = Some(pleochroma::optics::materials::ametrine_zoning());
    job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
    assert_eq!(format_for(&job), 5);
    assert!(job.validate().is_err(), "the file still says format 1");
    job.format = format_for(&job);
    assert_eq!(job.validate(), Ok(()));
    job.scene.tray_reflection = 0.04;
    job.scene.backdrop = 1.0;
    job.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    assert_eq!(format_for(&job), 5, "the zones' format wins");
    job.scene.tray_reflection = 0.0;
    job.scene.backdrop = 0.0;
    job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::ModelUnit;
    assert!(message(&job).contains("per-millimeter"));
    job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
    if let Some(zoning) = &mut job.zoning {
        zoning.boundary_softness_mm = -1.0;
    }
    assert!(message(&job).contains("The color zones are not usable"));
}

/// A turned HDR map needs format 6 whatever else the job holds: it wins over the zones' format 5,
/// and a file that still says an older format is refused.
#[test]
fn a_turned_hdr_job_requires_format_six() {
    let mut job = turned_hdr_still_job();
    assert_eq!(format_for(&job), 6);
    assert_eq!(job.format, 6);
    assert_eq!(job.validate(), Ok(()));
    job.format = 1;
    assert!(message(&job).contains("format 1"));
    job.format = 5;
    assert!(message(&job).contains("format 5"));

    job.zoning = Some(pleochroma::optics::materials::ametrine_zoning());
    job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
    assert_eq!(
        format_for(&job),
        6,
        "the turned map's format wins over the zones"
    );
    job.format = 6;
    assert_eq!(job.validate(), Ok(()));
    job.scene.hdr_orientation = pleochroma::renderer::env_map::HdrOrientation::default();
    assert_eq!(
        format_for(&job),
        5,
        "unturned, the zones' format shows again"
    );
}

/// A camera pan needs format 7 whatever else the job holds, since an older reader would render
/// the stone centred: it wins over a turned map's format 6, and a file that still says an older
/// format is refused.
#[test]
fn a_panned_job_requires_format_seven() {
    let mut job = panned_still_job();
    assert_eq!(format_for(&job), 7);
    assert_eq!(job.format, 7);
    assert_eq!(job.validate(), Ok(()));
    job.format = 6;
    assert!(message(&job).contains("format 6"));

    let mut turned = turned_hdr_still_job();
    turned.scene.camera_pan = [0.1, 0.0];
    assert_eq!(
        format_for(&turned),
        7,
        "the pan's format wins over the turned map"
    );
    turned.scene.camera_pan = [0.0, 0.0];
    assert_eq!(
        format_for(&turned),
        6,
        "centred, the map's format shows again"
    );
}

/// Only a turned map in an HDR scene makes format 6: a yaw or the camera flag alone does. An
/// unturned map, or a studio scene with a stray orientation, does not, and an unturned job
/// writes no `hdr_orientation` key.
#[test]
fn an_unturned_or_studio_job_keeps_its_format_and_writes_no_orientation_key() {
    use pleochroma::renderer::env_map::HdrOrientation;

    let (scene, source) = hdr_scene();
    let mut job = still_job();
    job.scene = scene;
    job.hdr = Some(source);
    assert_eq!(format_for(&job), 1);
    assert_eq!(job.validate(), Ok(()));
    assert!(
        !crate::codec::to_text(&job)
            .unwrap()
            .contains("hdr_orientation")
    );

    job.scene.hdr_orientation = HdrOrientation {
        yaw: 0.5,
        follows_camera: false,
    };
    assert_eq!(format_for(&job), 6, "a yaw alone turns the map");
    job.scene.hdr_orientation = HdrOrientation {
        yaw: 0.0,
        follows_camera: true,
    };
    assert_eq!(
        format_for(&job),
        6,
        "following the camera alone turns the map"
    );
    job.format = 6;
    assert_eq!(job.validate(), Ok(()));
    assert!(
        crate::codec::to_text(&job)
            .unwrap()
            .contains("hdr_orientation")
    );

    let mut studio = still_job();
    studio.scene.hdr_orientation = HdrOrientation {
        yaw: 0.5,
        follows_camera: true,
    };
    assert_eq!(
        format_for(&studio),
        1,
        "a studio scene ignores the map's turn"
    );
    assert_eq!(studio.validate(), Ok(()));
}

/// The scene's own zones move into the file's field, and a Ray-Traced job drops them.
#[test]
fn moving_the_scene_zoning_into_the_file_clears_the_material() {
    let zoning = pleochroma::optics::materials::ametrine_zoning();
    let mut job = still_job();
    job.scene.material.zoning = Some(zoning.clone());
    job.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
    job.move_scene_zoning_to_file();
    assert_eq!(job.zoning, Some(zoning.clone()));
    assert!(job.scene.material.zoning.is_none());

    let mut ray = still_job();
    ray.engine.engine = pleochroma::renderer::engine::RenderEngine::RayTraced;
    ray.scene.material.zoning = Some(zoning);
    ray.move_scene_zoning_to_file();
    assert!(
        ray.zoning.is_none(),
        "the Ray-Traced engine renders the base color"
    );
    assert!(ray.scene.material.zoning.is_none());

    // A hand-edited Ray-Traced file that carries zones is refused, not rendered without them.
    ray.scene.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
    ray.zoning = Some(pleochroma::optics::materials::ametrine_zoning());
    ray.format = format_for(&ray);
    assert!(message(&ray).contains("Color zones require Path-Traced rendering."));
}

/// A job without the shadow writes no `stone_shadow` key.
#[test]
fn a_shadow_free_job_writes_no_stone_shadow_key() {
    let job = still_job();
    let value = serde_json::to_value(&job).unwrap();
    let scene = value
        .get("scene")
        .and_then(serde_json::Value::as_object)
        .expect("the job has a scene object");
    assert!(!scene.contains_key("stone_shadow"));
    assert!(
        !crate::codec::to_text(&job)
            .unwrap()
            .contains("stone_shadow")
    );

    let mut shadowed = still_job();
    shadowed.scene.stone_shadow = true;
    shadowed.scene.backdrop = 1.0;
    shadowed.scene.lighting_preset = pleochroma::optics::raytracer::LightingPreset::LightTent;
    shadowed.format = format_for(&shadowed);
    assert!(
        crate::codec::to_text(&shadowed)
            .unwrap()
            .contains("stone_shadow")
    );
}
