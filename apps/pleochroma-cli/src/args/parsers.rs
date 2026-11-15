//! One parser per command: the words after the command's name into its argument struct.

#[cfg(feature = "adaptive-sampling")]
use super::types::AdaptiveArgs;
#[cfg(feature = "beam")]
use super::types::{
    BEAM_NODES_RANGE, BEAM_PERSISTENCE_AVAILABLE, BeamArgs, MAX_BEAM_BATCH_TILES,
    MAX_BEAM_RESIDUAL_MIN_SPP, keep_beams_refusal,
};
use super::{
    batch::{Fan, fan_out},
    types::{
        CensusArgs, CensusBuiltins, CensusOutput, Command, ExportArgs, ExportFormat, Exposure,
        InfoArgs, JobCommandKind, MaterialArg, MetricsArgs, Mode, OptimizeArgs, PathCacheArgs,
        RenderJobArgs, ReportFormat, ReportStyle, RetargetArgs, SnapLimit, SnapMeetsArgs,
        SolveArgs, SweepArgs, ValidateArgs,
    },
    values::{
        Cursor, parse_candidates, parse_count, parse_crown_fraction, parse_crown_mode,
        parse_export_format, parse_ids, parse_lighting, parse_max_move_fraction, parse_max_move_mm,
        parse_mode, parse_modes, parse_preset, parse_range, parse_ri, parse_targets, set_material,
    },
};
#[cfg(feature = "beam")]
use pleochroma::renderer::accel_beam::{BeamOff, RegionMode};
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::NoiseMetric;
use pleochroma::{color::HighlightRule, optics::tables::OpticsTables};
use pleochroma_cut_core::{
    CANONICAL_LIGHTING_PRESET, DEFAULT_SNAP_MAX_MOVE_FRACTION, ObjectivePreset,
};
use pleochroma_editor::{
    optimize_view::{
        DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS, parse_budget, parse_seed, parse_starts,
    },
    retarget::CrownShift,
};
use pleochroma_formats::native::design::{DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED};
use pleochroma_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::PathBuf;

pub(super) fn parse_info(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("info", args);
    let (mut designs, mut json, mut out, mut out_dir, mut db) =
        (Vec::new(), false, None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "info",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: if json { "json" } else { "txt" },
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Info(InfoArgs {
            design,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_solve(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("solve", args);
    let (mut designs, mut json, mut out, mut out_dir, mut db) =
        (Vec::new(), false, None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "solve",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Solve(SolveArgs {
            design,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_metrics(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("metrics", args);
    let mut designs = Vec::new();
    let mut material = None;
    let mut tilt = false;
    let (mut json, mut csv) = (false, false);
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--tilt" => tilt = true,
            "--json" => json = true,
            "--csv" => csv = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    if json && csv {
        return Err("give one of --json and --csv, not both".to_string());
    }
    let (format, extension, style) = if json {
        (ReportFormat::Json, "json", ReportStyle::Json)
    } else if csv {
        (ReportFormat::Csv, "csv", ReportStyle::Csv)
    } else {
        (ReportFormat::Text, "txt", ReportStyle::Text)
    };
    let fan = Fan {
        command: "metrics",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension,
        style,
    };
    fan_out(fan, |design, out| {
        Command::Metrics(MetricsArgs {
            design,
            material: material.clone(),
            tilt,
            format,
            lighting,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_validate(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("validate", args);
    let mut designs = Vec::new();
    let mut material = None;
    let mut json = false;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--json" => json = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "validate",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: if json { "json" } else { "txt" },
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Validate(ValidateArgs {
            design,
            json,
            material: material.clone(),
            lighting,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_optimize(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("optimize", args);
    let mut designs = Vec::new();
    let mut preset = ObjectivePreset::Balanced;
    let mut budget = DEFAULT_BUDGET;
    let mut starts = DEFAULT_STARTS;
    let mut seed = 0_u64;
    let mut vary_anchored = false;
    let mut candidates = DEFAULT_CANDIDATES;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--preset" => preset = parse_preset(cursor.value("--preset")?)?,
            "--budget" => budget = parse_budget(cursor.value("--budget")?)?,
            "--starts" => starts = parse_starts(cursor.value("--starts")?)?,
            "--seed" => seed = parse_seed(cursor.value("--seed")?)?,
            "--vary-anchored" => vary_anchored = true,
            "--candidates" => candidates = parse_candidates(cursor.value("--candidates")?)?,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "optimize",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Optimize(OptimizeArgs {
            design,
            preset,
            budget,
            starts,
            seed,
            vary_anchored,
            candidates,
            lighting,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_retarget(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("retarget", args);
    let mut designs = Vec::new();
    let mut target = None;
    let mut mode = Mode::Shift;
    let mut crown = CrownShift::default();
    let (mut fraction_given, mut ratio_given, mut follow_given) = (false, false, false);
    let mut preset = ObjectivePreset::Balanced;
    let mut range_deg = None;
    let mut budget = None;
    let mut seed = 0_u64;
    let mut keep_look = true;
    let mut girdle_allowance = false;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut target, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut target, MaterialArg::Ri(ri))?;
            }
            "--mode" => mode = parse_mode(cursor.value("--mode")?)?,
            "--crown-fraction" => {
                crown.fraction = parse_crown_fraction(cursor.number("--crown-fraction")?)?;
                // An explicit rule: it replaces the default of following the pavilion.
                crown.follow_pavilion = false;
                fraction_given = true;
            }
            "--crown-ratio" => {
                crown.scale_by_ratio = true;
                crown.follow_pavilion = false;
                ratio_given = true;
            }
            // The default, accepted so a script can say so.
            "--crown-follow" => {
                crown.follow_pavilion = true;
                follow_given = true;
            }
            "--preset" => preset = parse_preset(cursor.value("--preset")?)?,
            "--range" => range_deg = Some(parse_range(cursor.number("--range")?)?),
            "--budget" => budget = Some(parse_budget(cursor.value("--budget")?)?),
            "--seed" => seed = parse_seed(cursor.value("--seed")?)?,
            "--no-keep-look" => keep_look = false,
            "--girdle-allowance" => girdle_allowance = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    if usize::from(fraction_given) + usize::from(ratio_given) + usize::from(follow_given) > 1 {
        return Err(
            "give one of --crown-follow, --crown-fraction F and --crown-ratio, not several"
                .to_string(),
        );
    }
    let designs = cursor.require_designs(designs)?;
    let target = target.ok_or_else(|| {
        "\"retarget\" needs the material to retarget for: --material NAME or --ri N (see --help)"
            .to_string()
    })?;
    let fan = Fan {
        command: "retarget",
        designs,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Retarget(RetargetArgs {
            design,
            target: target.clone(),
            mode,
            crown,
            preset,
            range_deg,
            budget,
            seed,
            keep_look,
            girdle_allowance,
            lighting,
            json,
            out,
            db: db.clone(),
        })
    })
}

/// The materials `census` retargets to when `--targets` is not given.
const DEFAULT_CENSUS_TARGETS: [&str; 5] = [
    "Diamond",
    "Sapphire",
    "Quartz",
    "Cubic Zirconia",
    "Tourmaline",
];

/// The search budget `census` gives the optimize mode when `--budget` is not given.
const DEFAULT_CENSUS_BUDGET: usize = 100;

pub(super) fn parse_census(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("census", args);
    let mut files = Vec::new();
    let mut builtins = CensusBuiltins::default();
    let mut output = CensusOutput::default();
    let mut all = false;
    let (mut db, mut out, mut out_dir) = (None, None, None);
    let mut ids = Vec::new();
    let (mut limit, mut offset) = (None, 0_usize);
    let mut targets = None;
    let mut modes = None;
    let mut crown = CrownShift::default();
    let mut budget = DEFAULT_CENSUS_BUDGET;
    while let Some(word) = cursor.token() {
        match word {
            "--templates" => builtins.templates = true,
            "--fixtures" => builtins.fixtures = true,
            "--all" => all = true,
            "--timings" => output.timings = true,
            "--json" => output.json = true,
            "--db" => db = Some(cursor.path("--db")?),
            "--ids" => ids = parse_ids(cursor.value("--ids")?)?,
            "--limit" => limit = Some(parse_count("--limit", cursor.value("--limit")?)?),
            "--offset" => offset = parse_count("--offset", cursor.value("--offset")?)?,
            "--targets" => targets = Some(parse_targets(cursor.value("--targets")?)?),
            "--modes" => modes = Some(parse_modes(cursor.value("--modes")?)?),
            "--crown" => crown = parse_crown_mode(cursor.value("--crown")?)?,
            "--budget" => budget = parse_budget(cursor.value("--budget")?)?,
            "--out" => out = Some(cursor.path("--out")?),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            other => cursor.design_word(other, &mut files)?,
        }
    }
    if all && !ids.is_empty() {
        return Err("give one of --all and --ids, not both".to_string());
    }
    if (all || !ids.is_empty()) && db.is_none() {
        return Err("--all and --ids read a design library: give --db FILE".to_string());
    }
    if db.is_some() && !all && ids.is_empty() {
        return Err("--db needs --all or --ids LIST for the designs to measure".to_string());
    }
    if files.is_empty() && builtins == CensusBuiltins::default() && db.is_none() {
        return Err(
            "\"census\" needs designs to measure: --templates, --fixtures, design files or \
             --db FILE with --all or --ids (see --help)"
                .to_string(),
        );
    }
    Ok(Command::Census(CensusArgs {
        files,
        builtins,
        db,
        all,
        ids,
        limit,
        offset,
        targets: targets.unwrap_or_else(|| {
            DEFAULT_CENSUS_TARGETS
                .iter()
                .map(|name| MaterialArg::Named((*name).to_string()))
                .collect()
        }),
        modes: modes.unwrap_or_else(|| vec![Mode::Shift]),
        crown,
        budget,
        output,
        out,
        out_dir,
    }))
}

pub(super) fn parse_sweep(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("sweep", args);
    let mut designs = Vec::new();
    let mut tier = None;
    let (mut from_deg, mut to_deg, mut step_deg) = (None, None, None);
    let mut tilt = false;
    let (mut csv, mut out_dir) = (None, None);
    let mut json = false;
    let mut material = None;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut db = None;
    while let Some(word) = cursor.token() {
        match word {
            "--tier" => tier = Some(cursor.value("--tier")?.to_string()),
            "--from" => from_deg = Some(cursor.number("--from")?),
            "--to" => to_deg = Some(cursor.number("--to")?),
            "--step" => step_deg = Some(cursor.number("--step")?),
            "--tilt" => tilt = true,
            "--csv" => csv = Some(("--csv", cursor.path("--csv")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--json" => json = true,
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let designs = cursor.require_designs(designs)?;
    let missing = |flag: &str| format!("\"sweep\" needs {flag} (see --help)");
    let tier = tier.ok_or_else(|| missing("--tier NAME"))?;
    let from_deg = from_deg.ok_or_else(|| missing("--from ANGLE"))?;
    let to_deg = to_deg.ok_or_else(|| missing("--to ANGLE"))?;
    let step_deg = step_deg.ok_or_else(|| missing("--step DEGREES"))?;
    let fan = Fan {
        command: "sweep",
        designs,
        out: csv,
        out_dir,
        extension: "csv",
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, csv| {
        Command::Sweep(SweepArgs {
            design,
            tier: tier.clone(),
            from_deg,
            to_deg,
            step_deg,
            tilt,
            csv,
            json,
            material: material.clone(),
            lighting,
            db: db.clone(),
        })
    })
}

/// `snap-meets`: the design files, the limit (the default share of the stone's size unless
/// `--max-move` or `--max-move-fraction` gives one) and where the result goes. An output
/// (`--out` or `--out-dir`) is required unless `--dry-run` is given.
pub(super) fn parse_snap_meets(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("snap-meets", args);
    let mut designs = Vec::new();
    let (mut max_move_mm, mut max_move_fraction) = (None, None);
    let mut dry_run = false;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--max-move" => {
                max_move_mm = Some(parse_max_move_mm(cursor.number("--max-move")?)?);
            }
            "--max-move-fraction" => {
                max_move_fraction = Some(parse_max_move_fraction(
                    cursor.number("--max-move-fraction")?,
                )?);
            }
            "--dry-run" => dry_run = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let designs = cursor.require_designs(designs)?;
    let limit = match (max_move_mm, max_move_fraction) {
        (Some(_), Some(_)) => {
            return Err(
                "give one of --max-move MM and --max-move-fraction F, not both".to_string(),
            );
        }
        (Some(mm), None) => SnapLimit::Millimeters(mm),
        (None, Some(fraction)) => SnapLimit::Fraction(fraction),
        (None, None) => SnapLimit::Fraction(DEFAULT_SNAP_MAX_MOVE_FRACTION),
    };
    if !dry_run && out.is_none() && out_dir.is_none() {
        return Err(format!(
            "\"snap-meets\" needs --out FILE{DESIGN_EXTENSION_DOTTED} (--out-dir DIR for several \
             designs), or --dry-run to only report what would move (see --help)"
        ));
    }
    let fan = Fan {
        command: "snap-meets",
        designs,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::Text,
    };
    fan_out(fan, |design, out| {
        Command::SnapMeets(SnapMeetsArgs {
            design,
            limit,
            dry_run,
            out,
            db: db.clone(),
        })
    })
}

fn parse_local(word: &str) -> Result<LocalEngines, String> {
    LocalEngines::from_cli_word(word)
        .ok_or_else(|| format!("unknown engines {word:?} (expected cpu, gpu or cpu+gpu)"))
}

fn parse_compute(word: &str) -> Result<ComputeChoice, String> {
    ComputeChoice::from_cli_word(word)
        .ok_or_else(|| format!("unknown compute choice {word:?} (expected local, remote or both)"))
}

fn parse_transfer(word: &str) -> Result<TransferChoice, String> {
    TransferChoice::from_cli_word(word)
        .ok_or_else(|| format!("unknown transfer choice {word:?} (expected full or final)"))
}

fn parse_highlights(word: &str) -> Result<HighlightRule, String> {
    HighlightRule::from_cli_word(word)
        .ok_or_else(|| format!("unknown highlight rule {word:?} (expected whiten or keep-color)"))
}

/// The pattern `--color-pattern` names; whether the material has it is judged by the command.
fn parse_pattern_name(word: &str) -> Result<String, String> {
    let name = word.trim();
    if name.is_empty() {
        return Err("--color-pattern needs a pattern name, got an empty value".to_string());
    }
    Ok(name.to_string())
}

/// The name `--material` gives; whether it is a built-in material is judged by the command.
fn parse_material_name(word: &str) -> Result<String, String> {
    let name = word.trim();
    if name.is_empty() {
        return Err("--material needs a material name, got an empty value".to_string());
    }
    Ok(name.to_string())
}

/// The key `--color-variant` names; whether the job's material has it is judged by the command.
fn parse_variant_key(word: &str) -> Result<String, String> {
    let key = word.trim();
    if key.is_empty() {
        return Err("--color-variant needs a variant key, got an empty value".to_string());
    }
    Ok(key.to_string())
}

/// The Tray Reflection `--tray-reflection` gives, as the fraction the scene carries: a percent
/// from 0 (the matte tray) to 100 (a mirror), the unit of the app's slider.
fn parse_tray_reflection(word: &str) -> Result<f32, String> {
    let percent: f32 = word
        .trim()
        .parse()
        .map_err(|_| format!("--tray-reflection needs a percent from 0 to 100, got {word:?}"))?;
    if !(0.0..=100.0).contains(&percent) {
        return Err(format!(
            "--tray-reflection needs a percent from 0 to 100, got {word:?}"
        ));
    }
    Ok(percent / 100.0)
}

/// The Stone Pose `--stone-pose` gives: tilt and turn, or tilt, turn and spin, in degrees. Every
/// value must be a finite number; the range is applied when the pose is laid over the job.
fn parse_stone_pose(word: &str) -> Result<[f32; 3], String> {
    let refusal = || "--stone-pose takes TILT,TURN or TILT,TURN,SPIN in degrees".to_string();
    let mut values = [0.0_f32; 3];
    let mut count = 0;
    for part in word.split(',') {
        let slot = values.get_mut(count).ok_or_else(refusal)?;
        let value: f32 = part.trim().parse().map_err(|_| refusal())?;
        if !value.is_finite() {
            return Err(refusal());
        }
        *slot = value;
        count += 1;
    }
    if count < 2 {
        return Err(refusal());
    }
    Ok(values)
}

/// The renderer `--engine` names: `path` or `ray`.
fn parse_engine(word: &str) -> Result<pleochroma::renderer::engine::RenderEngine, String> {
    match word {
        "path" => Ok(pleochroma::renderer::engine::RenderEngine::PathTraced),
        "ray" => Ok(pleochroma::renderer::engine::RenderEngine::RayTraced),
        other => Err(format!(
            "unknown renderer {other:?} for --engine (expected path or ray)"
        )),
    }
}

/// The `HOST:PORT` `--remote` gives; whether a worker answers there is found out by the command.
fn parse_remote_address(word: &str) -> Result<String, String> {
    let address = word.trim();
    if address.is_empty() {
        return Err("--remote needs HOST:PORT, got an empty value".to_string());
    }
    Ok(address.to_string())
}

/// `render` or `tilt-video` before any flag is read: every option at its default.
fn blank_render_job(kind: JobCommandKind) -> RenderJobArgs {
    RenderJobArgs {
        stone_shadow: None,
        tray_reflection: None,
        stone_pose: None,
        engine: None,
        kind,
        job: PathBuf::new(),
        out: None,
        local: LocalEngines::default(),
        compute: None,
        transfer: None,
        remote: None,
        cert_dir: None,
        contribute_local: false,
        restart: false,
        quiet: false,
        highlights: HighlightRule::default(),
        material: None,
        color_variant: None,
        color_pattern: None,
        exposure: Exposure::AsJob,
        raw_out: None,
        progress_raw: None,
        sample_offset: 0,
        #[cfg(feature = "adaptive-sampling")]
        adaptive: AdaptiveArgs::default(),
        path_cache: PathCacheArgs::default(),
        optics_tables: OpticsTables::Off,
        #[cfg(feature = "beam")]
        beam: BeamArgs::default(),
        verbose: false,
    }
}

/// Parses the words of `render` or `tilt-video`: one job file and the engine options.
pub(super) fn parse_render_job(
    kind: JobCommandKind,
    args: &[String],
) -> Result<RenderJobArgs, String> {
    let command = kind.word();
    let mut cursor = Cursor::new(command, args);
    let mut parsed = blank_render_job(kind);
    let mut job: Option<PathBuf> = None;
    while let Some(word) = cursor.token() {
        if word.starts_with('-') {
            apply_render_flag(kind, word, &mut cursor, &mut parsed)?;
        } else if job.is_some() {
            return Err(format!(
                "unexpected extra argument {word:?} for \"{command}\": give one job file \
                 (see --help)"
            ));
        } else {
            job = Some(PathBuf::from(word));
        }
    }
    parsed.job = job.ok_or_else(|| format!("\"{command}\" needs a job file (see --help)"))?;
    check_render_job_flags(&parsed)?;
    Ok(parsed)
}

/// Reads one flag of `render` or `tilt-video` (with its value, if it takes one) into `parsed`.
fn apply_render_flag(
    kind: JobCommandKind,
    word: &str,
    cursor: &mut Cursor<'_>,
    parsed: &mut RenderJobArgs,
) -> Result<(), String> {
    if apply_accel_flag(word, cursor, parsed)? {
        return Ok(());
    }
    let command = kind.word();
    match word {
        "--raw-out" => parsed.raw_out = Some(cursor.path("--raw-out")?),
        "--progress-raw" if kind == JobCommandKind::Render => {
            parsed.progress_raw = Some(cursor.path("--progress-raw")?);
        }
        "--progress-raw" => {
            return Err(
                "--progress-raw is for \"render\", not \"tilt-video\": a video's frames are \
                 not built up in steps (see --help)"
                    .to_string(),
            );
        }
        "--sample-offset" => {
            parsed.sample_offset = parse_sample_offset(cursor.value("--sample-offset")?)?;
        }
        "--out" if kind == JobCommandKind::Render => parsed.out = Some(cursor.path("--out")?),
        "--out" => {
            return Err(
                "--out is for \"render\"; use --out-dir with \"tilt-video\" (see --help)"
                    .to_string(),
            );
        }
        "--out-dir" if kind == JobCommandKind::TiltVideo => {
            parsed.out = Some(cursor.path("--out-dir")?);
        }
        "--restart" if kind == JobCommandKind::TiltVideo => parsed.restart = true,
        "--out-dir" | "--restart" => {
            return Err(format!(
                "{word} is for \"tilt-video\", not \"{command}\" (see --help)"
            ));
        }
        "--stone-shadow" if kind == JobCommandKind::Render => {
            parsed.stone_shadow = Some(match cursor.value("--stone-shadow")? {
                "on" => true,
                "off" => false,
                other => return Err(format!("--stone-shadow needs on or off, got {other:?}")),
            });
        }
        "--stone-shadow" => {
            return Err(
                "--stone-shadow is for \"render\", not \"tilt-video\". Tilt videos do not show \
                 the stone shadow yet."
                    .to_string(),
            );
        }
        "--tray-reflection" if kind == JobCommandKind::Render => {
            parsed.tray_reflection =
                Some(parse_tray_reflection(cursor.value("--tray-reflection")?)?);
        }
        "--tray-reflection" => {
            return Err(
                "--tray-reflection is for \"render\", not \"tilt-video\". Tilt videos do not \
                 show the tray reflection yet."
                    .to_string(),
            );
        }
        "--stone-pose" if kind == JobCommandKind::Render => {
            parsed.stone_pose = Some(parse_stone_pose(cursor.value("--stone-pose")?)?);
        }
        "--stone-pose" => {
            return Err(
                "--stone-pose is for \"render\", not \"tilt-video\". Tilt videos turn the stone \
                 themselves."
                    .to_string(),
            );
        }
        "--engine" => parsed.engine = Some(parse_engine(cursor.value("--engine")?)?),
        "--local" => parsed.local = parse_local(cursor.value("--local")?)?,
        "--compute" => parsed.compute = Some(parse_compute(cursor.value("--compute")?)?),
        "--transfer" => parsed.transfer = Some(parse_transfer(cursor.value("--transfer")?)?),
        "--remote" => parsed.remote = Some(parse_remote_address(cursor.value("--remote")?)?),
        "--cert-dir" => parsed.cert_dir = Some(cursor.path("--cert-dir")?),
        "--contribute-local" => parsed.contribute_local = true,
        "--quiet" => parsed.quiet = true,
        "--verbose" => parsed.verbose = true,
        "--meter" => parsed.exposure = Exposure::Metered,
        "--highlights" => parsed.highlights = parse_highlights(cursor.value("--highlights")?)?,
        "--material" => {
            parsed.material = Some(parse_material_name(cursor.value("--material")?)?);
        }
        "--color-variant" => {
            parsed.color_variant = Some(parse_variant_key(cursor.value("--color-variant")?)?);
        }
        "--color-pattern" => {
            parsed.color_pattern = Some(parse_pattern_name(cursor.value("--color-pattern")?)?);
        }
        other => {
            return Err(format!(
                "unknown flag {other:?} for \"{command}\" (see --help)"
            ));
        }
    }
    Ok(())
}

/// Reads one of the acceleration flags (the optics tables, the path cache, adaptive sampling in a
/// build with that feature) into `parsed`; `false` when `word` is not one of them.
fn apply_accel_flag(
    word: &str,
    cursor: &mut Cursor<'_>,
    parsed: &mut RenderJobArgs,
) -> Result<bool, String> {
    match word {
        "--optics-tables" => parsed.optics_tables = OpticsTables::On,
        "--path-cache" => parsed.path_cache.enabled = true,
        "--persist-cache" => {
            parsed.path_cache.enabled = true;
            parsed.path_cache.persist = true;
        }
        "--cache-dir" => parsed.path_cache.cache_dir = Some(cursor.path("--cache-dir")?),
        #[cfg(feature = "beam")]
        "--beam" => parsed.beam.enabled = true,
        #[cfg(feature = "beam")]
        "--keep-beams" => {
            parsed.beam.enabled = true;
            parsed.beam.keep = true;
        }
        #[cfg(feature = "beam")]
        "--beam-residual-min-spp"
        | "--beam-batch-tiles"
        | "--beam-region-mode"
        | "--beam-nodes" => apply_beam_override(word, cursor, &mut parsed.beam)?,
        #[cfg(feature = "adaptive-sampling")]
        "--adaptive" => parsed.adaptive.enabled = true,
        #[cfg(feature = "adaptive-sampling")]
        "--gpu-convergence" => parsed.adaptive.gpu_convergence = true,
        #[cfg(feature = "adaptive-sampling")]
        "--target-noise" => {
            parsed.adaptive.target_noise =
                Some(parse_target_noise(cursor.value("--target-noise")?)?);
            parsed.adaptive.enabled = true;
        }
        #[cfg(feature = "adaptive-sampling")]
        "--min-spp" => {
            parsed.adaptive.min_spp = Some(parse_min_spp(cursor.value("--min-spp")?)?);
            parsed.adaptive.enabled = true;
        }
        #[cfg(feature = "adaptive-sampling")]
        "--noise-metric" => {
            parsed.adaptive.metric = Some(parse_noise_metric(cursor.value("--noise-metric")?)?);
            parsed.adaptive.enabled = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The rules that tie the flags of `render` and `tilt-video` together, checked once all are read.
fn check_render_job_flags(parsed: &RenderJobArgs) -> Result<(), String> {
    if parsed.remote.is_some() != parsed.cert_dir.is_some() {
        return Err("--remote and --cert-dir go together: give both or neither".to_string());
    }
    if matches!(
        parsed.compute,
        Some(ComputeChoice::Remote | ComputeChoice::Both)
    ) && parsed.remote.is_none()
    {
        return Err("--compute remote needs --remote and --cert-dir".to_string());
    }
    #[cfg(feature = "adaptive-sampling")]
    if parsed.adaptive.gpu_convergence && !parsed.adaptive.enabled {
        return Err(
            "--gpu-convergence moves the convergence check of --adaptive to the GPU: add \
             --adaptive (see --help)"
                .to_string(),
        );
    }
    let ray_traced = matches!(
        parsed.engine,
        Some(pleochroma::renderer::engine::RenderEngine::RayTraced)
    );
    #[cfg(feature = "adaptive-sampling")]
    if ray_traced && parsed.adaptive.enabled {
        return Err(
            "--adaptive is for the path-traced renderer: the Ray Traced engine's taps are a \
             fixed set, with no noise to converge"
                .to_string(),
        );
    }
    if ray_traced && parsed.path_cache.enabled {
        return Err(
            "--path-cache is for the path-traced renderer: the Ray Traced engine has no \
             light paths to cache"
                .to_string(),
        );
    }
    #[cfg(feature = "beam")]
    if ray_traced && parsed.beam.enabled {
        return Err(format!(
            "{} (--beam): the Ray Traced engine has no residual to sample",
            BeamOff::RayTracedResidual.message()
        ));
    }
    #[cfg(feature = "beam")]
    if let Some(why) = keep_beams_refusal(parsed.beam.keep, BEAM_PERSISTENCE_AVAILABLE) {
        return Err(why.to_string());
    }
    if ray_traced && parsed.optics_tables == OpticsTables::On {
        return Err(
            "--optics-tables is for the path-traced renderer: the Ray Traced engine does not \
             use the tables"
                .to_string(),
        );
    }
    Ok(())
}

/// Reads one of the beam tracing overrides (with its value) into `beam` and turns beam tracing
/// on, as the adaptive companions turn adaptive sampling on.
#[cfg(feature = "beam")]
fn apply_beam_override(
    word: &str,
    cursor: &mut Cursor<'_>,
    beam: &mut BeamArgs,
) -> Result<(), String> {
    match word {
        "--beam-residual-min-spp" => {
            beam.residual_min_spp = Some(parse_beam_count(
                word,
                cursor.value(word)?,
                1,
                MAX_BEAM_RESIDUAL_MIN_SPP,
            )?);
        }
        "--beam-batch-tiles" => {
            beam.batch_tiles = Some(parse_beam_count(
                word,
                cursor.value(word)?,
                1,
                MAX_BEAM_BATCH_TILES,
            )?);
        }
        "--beam-region-mode" => beam.region_mode = Some(parse_region_mode(cursor.value(word)?)?),
        _ => beam.nodes = Some(parse_beam_nodes(cursor.value(word)?)?),
    }
    beam.enabled = true;
    Ok(())
}

/// A whole number from `least` to `most` for the beam flag `flag`.
#[cfg(feature = "beam")]
fn parse_beam_count(flag: &str, word: &str, least: u32, most: u32) -> Result<u32, String> {
    word.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (least..=most).contains(value))
        .ok_or_else(|| format!("{flag} needs a whole number from {least} to {most}, got {word:?}"))
}

/// The spectral nodes `--beam-nodes` names: a multiple of 8 within [`BEAM_NODES_RANGE`].
#[cfg(feature = "beam")]
fn parse_beam_nodes(word: &str) -> Result<u32, String> {
    let (least, most) = BEAM_NODES_RANGE;
    word.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| (least..=most).contains(value) && value.is_multiple_of(8))
        .ok_or_else(|| {
            format!(
                "--beam-nodes needs a multiple of 8 from {least} to {most} (default 48), \
                 got {word:?}"
            )
        })
}

/// The mode `--beam-region-mode` names: `end-intersection` or `per-node`.
#[cfg(feature = "beam")]
fn parse_region_mode(word: &str) -> Result<RegionMode, String> {
    match word.trim() {
        "end-intersection" => Ok(RegionMode::EndIntersection),
        "per-node" => Ok(RegionMode::PerNode),
        other => Err(format!(
            "unknown region mode {other:?} for --beam-region-mode (expected end-intersection or \
             per-node)"
        )),
    }
}

/// The error `--target-noise` names: a number above 0. Whether it fits the metric (below 1 for
/// the relative one) is judged with the metric and the job, by [`AdaptiveArgs::settings`].
#[cfg(feature = "adaptive-sampling")]
fn parse_target_noise(word: &str) -> Result<f32, String> {
    word.trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            format!(
                "--target-noise needs a number above 0 (a fraction such as 0.01 for the relative \
                 metric, a Delta E such as 1.5 for the absolute one), got {word:?}"
            )
        })
}

/// The samples `--min-spp` names: a whole number from 1 up.
#[cfg(feature = "adaptive-sampling")]
fn parse_min_spp(word: &str) -> Result<u32, String> {
    word.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value >= 1)
        .ok_or_else(|| {
            format!(
                "--min-spp needs a whole number from 1 up (samples per pixel before a tile may \
                 stop), got {word:?}"
            )
        })
}

/// The metric `--noise-metric` names: `relative` or `absolute`.
#[cfg(feature = "adaptive-sampling")]
fn parse_noise_metric(word: &str) -> Result<NoiseMetric, String> {
    match word.trim() {
        "relative" => Ok(NoiseMetric::RelativeLstar),
        "absolute" => Ok(NoiseMetric::AbsoluteDeltaE),
        other => Err(format!(
            "unknown noise metric {other:?} for --noise-metric (expected relative or absolute)"
        )),
    }
}

/// The first sample index `--sample-offset` names: a whole number from 0 up.
fn parse_sample_offset(word: &str) -> Result<u32, String> {
    word.trim().parse::<u32>().map_err(|_| {
        format!(
            "--sample-offset needs a whole number from 0 up (the first sample index), \
             got {word:?}"
        )
    })
}

pub(super) fn parse_export(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("export", args);
    let mut designs = Vec::new();
    let mut format = None;
    let mut out: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut db = None;
    let mut date = None;
    while let Some(word) = cursor.token() {
        match word {
            "--format" => format = Some(parse_export_format(cursor.value("--format")?)?),
            "--out" => out = Some(cursor.path("--out")?),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            "--date" => date = Some(cursor.value("--date")?.to_string()),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let designs = cursor.require_designs(designs)?;
    if out.is_none() && out_dir.is_none() {
        return Err(
            "\"export\" needs --out FILE, or --out-dir DIR for several designs (see --help)"
                .to_string(),
        );
    }
    // `--out-dir` names no file, so the format cannot come from the name there.
    let format = format
        .or_else(|| {
            out.as_ref()
                .and_then(|path| ExportFormat::from_file_name(&path.to_string_lossy()))
        })
        .ok_or_else(|| {
            format!(
                "\"export\" needs --format asc|{DESIGN_EXTENSION}|gcs|html|pdf, or an --out file \
                 ending in .asc, {DESIGN_EXTENSION_DOTTED}, .gcs, .html or .pdf (see --help)"
            )
        })?;
    let fan = Fan {
        command: "export",
        designs,
        out: out.map(|file| ("--out", file)),
        out_dir,
        extension: format.extension(),
        style: ReportStyle::Text,
    };
    fan_out(fan, |design, out| {
        Command::Export(ExportArgs {
            design,
            format,
            // Always given: the line named `--out`, or `--out-dir` made one for each design.
            out: out.unwrap_or_default(),
            db: db.clone(),
            date: date.clone(),
        })
    })
}
