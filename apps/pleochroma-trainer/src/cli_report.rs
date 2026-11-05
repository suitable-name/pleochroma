//! What the command-line trainer prints: the banner, completed-iteration metrics and the
//! end-of-run summary.
//!
//! The figures come from the run's own telemetry ([`IterationMetricPoint`]) and from the
//! validation report, and the phrases are built by small pure functions, so the algorithm-specific
//! parts (imitation shows loss, cosine and labels, PPO the critic's explained variance) can be
//! tested without a run.

use pleochroma_ai::{model::TrainingMode, training::LrSchedule};

use crate::{
    cli_args::CliArgs,
    cli_console::Console,
    dataset::MINERAL_SPECTRUM,
    runner::{Algorithm, ObjectiveSampling, RunConfig, TrainingRunner},
    stats::{IterationMetricPoint, describe_drops, drop_ratio},
};

/// Renders a sparkline of the last `max_chars` values in plain ASCII (block glyphs come out as
/// garbage on terminals without a Unicode font or charset, `PuTTY` among them).
fn render_sparkline(values: &[f32], max_chars: usize) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let slice = &values[values.len().saturating_sub(max_chars)..];
    let min = slice.iter().copied().fold(f32::INFINITY, f32::min);
    let max = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-4);
    let blocks = ['_', '.', ':', '-', '=', '+', '*', '#'];
    slice
        .iter()
        .map(|&v| {
            let norm = ((v - min) / range).clamp(0.0, 1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "norm is clamped to 0..=1, so the rounded index is a small non-negative count"
            )]
            let idx = ((norm * (blocks.len() - 1) as f32).round() as usize).min(blocks.len() - 1);
            blocks[idx]
        })
        .collect()
}

/// The number of logical cores the process can use (at least 1).
fn core_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// The banner phrase of the parallel environments: `16 (cores 32)`.
fn describe_workers(workers: usize, cores: usize) -> String {
    format!("{workers} (cores {cores})")
}

/// The scales of a line search as one banner phrase: `1 / 0.5 / 0.25`.
fn describe_line_search(scales: &[f64]) -> String {
    scales
        .iter()
        .map(f64::to_string)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The training algorithm as one banner phrase.
const fn describe_algorithm(algo: Algorithm) -> &'static str {
    match algo {
        Algorithm::Imitation => {
            "imitation (label the rollout states with finite differences, regress onto them)"
        }
        Algorithm::Ppo => "ppo (clipped policy gradient on the step reward)",
    }
}

/// The objective sampling as one banner phrase.
const fn describe_objectives(sampling: ObjectiveSampling) -> &'static str {
    match sampling {
        ObjectiveSampling::Balanced => "balanced (the default weights in every episode)",
        ObjectiveSampling::Presets => "presets (uniform over all seven objective presets)",
        ObjectiveSampling::Mixed => "mixed (presets 70 %, random weights 30 %)",
    }
}

/// The label rule as one banner phrase; `step_limit_deg` is the probe size when the run sets
/// none.
fn describe_labels(config: &RunConfig, step_limit_deg: f64) -> String {
    if config.label_every == 0 {
        return "off".to_owned();
    }
    let h = config.label_h_deg.unwrap_or(step_limit_deg);
    let rule = format!(
        "1 in {} rollout samples, probe h = {h:.2} deg",
        config.label_every
    );
    match config.algo {
        Algorithm::Imitation => {
            format!(
                "{rule}, up to {} compact rows within {} MiB host replay budget (actual capacity depends on tier counts)",
                config.label_capacity, config.label_memory_mib
            )
        }
        Algorithm::Ppo => format!("{rule} (telemetry only, the PPO loss ignores them)"),
    }
}

/// The learning-rate schedule as one banner phrase.
fn describe_schedule(schedule: LrSchedule) -> String {
    match schedule {
        LrSchedule::Constant => "constant".to_owned(),
        LrSchedule::Linear { total_updates } => {
            format!("linear decay to 0 over {total_updates} updates")
        }
    }
}

/// The rollout banner phrase: the steps per rollout, the mini-batch and how much the update
/// trains. Imitation trains `updates_per_round` mini-batches per round, or, when that is 0, the
/// old `epochs_per_round` full epochs over the aggregate; PPO trains epochs per iteration.
fn describe_rollout(config: &RunConfig) -> String {
    let steps = config.rollout_size;
    match config.algo {
        Algorithm::Imitation if config.imitation.updates_per_round > 0 => format!(
            "{steps} steps, {} updates per round, mini-batch {}",
            config.imitation.updates_per_round, config.imitation.mini_batch_size
        ),
        Algorithm::Imitation => format!(
            "{steps} steps, mini-batch {}, {} epochs per round",
            config.imitation.mini_batch_size, config.imitation.epochs_per_round
        ),
        Algorithm::Ppo => format!(
            "{steps} steps, mini-batch {}, {} epochs per iteration",
            config.ppo.mini_batch_size, config.ppo.epochs_per_batch
        ),
    }
}

/// Prints the run configuration; `sizes` is the (train, val, test) design count.
pub fn print_banner(
    runner: &TrainingRunner,
    args: &CliArgs,
    sizes: (usize, usize, usize),
    max_tiers: usize,
) {
    let (train, val, test) = sizes;
    let tier = args.tier;
    println!("PLEOCHROMA RL TRAINER");
    println!(
        "  model        size {} (~{} parameters, {} layers, {} heads)",
        tier.code_str(),
        tier.param_count_approx(),
        tier.config().2,
        tier.config().1
    );
    println!("  backend      {}", runner.device_label());
    println!("  mode         {}", args.mode);
    println!(
        "  workers      {} configured, {} effective rollout slots; {} persistent CPU pool threads",
        describe_workers(runner.config().workers, core_count()),
        runner.config().effective_workers(),
        crate::runner::cpu_pool_threads()
    );
    if cfg!(debug_assertions) {
        println!(
            "  warning      debug assertions enabled; training builds may run substantially slower"
        );
    }
    print_tuning(runner, args);
    println!("  data set     {train} train / {val} val / {test} test, padded to {max_tiers} tiers");
    if val == 0 {
        println!("  warning      validation split is empty: no validation will run");
    }
    if args.mode == TrainingMode::Retarget {
        if args.spectrum {
            println!(
                "  targets      whole spectrum, n {:.2} to {:.2}",
                MINERAL_SPECTRUM.first().map_or(0.0, |m| m.refractive_index),
                MINERAL_SPECTRUM.last().map_or(0.0, |m| m.refractive_index)
            );
        } else {
            println!("  target       {}", args.target_material);
        }
    }
    println!("  scores       0-100, lower is better\n");
}

/// Banner lines of the algorithm and the settings in effect that decide how the run learns:
/// rollout, step limit, label rule, line search, objectives, exploration, learning rate and
/// schedule, and the validation cadence.
fn print_tuning(runner: &TrainingRunner, args: &CliArgs) {
    let config = runner.config();
    println!("  algorithm    {}", describe_algorithm(config.algo));
    println!("  rollout      {}", describe_rollout(config));
    println!(
        "  step limit   {:.2} degrees per tier and step ({})",
        runner.max_step_deg(),
        if args.max_step_deg.is_some() {
            "--max-step-deg"
        } else {
            "environment default"
        }
    );
    println!(
        "  labels       {}",
        describe_labels(config, runner.max_step_deg())
    );
    println!(
        "  line search  {}",
        describe_line_search(&config.line_search)
    );
    println!("  objectives   {}", describe_objectives(config.objectives));
    println!(
        "  fidelity     {:?}; {:.0}% Full tilt training episodes",
        config.train_fidelity,
        if config.train_fidelity == pleochroma_cut_core::ObjectiveFidelity::Full {
            100.0
        } else {
            config.full_fidelity_frac * 100.0
        }
    );
    if config.algo == Algorithm::Imitation {
        println!(
            "  exploration  std {} on the policy mean",
            config.explore_std
        );
        if config.max_rejected_steps > 0 {
            println!(
                "  stalled slot restarts after {} consecutive rejected steps (training only)",
                config.max_rejected_steps
            );
        } else {
            println!("  stalled slot early restart disabled");
        }
    }
    if config.mode == TrainingMode::Retarget {
        println!(
            "  same material {:.0} % target the episode's source (authored sources are preserved)",
            config.same_material_frac * 100.0
        );
    }
    print_learning(config);
    println!(
        "  validation   every {} iterations on {} stratified held-out designs (model and sampled; \
         Fast also computes an oracle once)",
        config.validation_every, config.validation_designs
    );
}

/// Banner line of the learning rate, its schedule and the loss weights of the algorithm.
fn print_learning(config: &RunConfig) {
    match config.algo {
        Algorithm::Imitation => println!(
            "  learning     rate {:e}, {}; gradient-norm clip {}; cosine coeff {}, action coeff {}, gain coeff {}",
            config.imitation.learning_rate,
            describe_schedule(config.imitation.lr_schedule),
            config.imitation.max_grad_norm,
            config.imitation.cosine_coeff,
            config.imitation.action_coeff,
            config.imitation.gain_coeff
        ),
        Algorithm::Ppo => println!(
            "  learning     rate {:e}, {}; entropy bonus {}",
            config.ppo.learning_rate,
            describe_schedule(config.ppo.lr_schedule),
            config.ppo.entropy_coeff
        ),
    }
}

/// The figures that tell how the update is going, as one progress-line phrase: loss, direction
/// cosine and aggregate size for imitation, the critic's explained variance for PPO.
fn learning_figures(point: &IterationMetricPoint, imitation: bool) -> String {
    if imitation {
        // The first iteration has no update yet: its loss is a default, not a measurement.
        let loss = if point.has_update() {
            format!("{:.3}", point.imitation_loss)
        } else {
            "-".to_owned()
        };
        let action = if point.has_update() {
            format!("{:.3}", point.action_loss)
        } else {
            "-".to_owned()
        };
        format!(
            "loss {loss} | cos {} | action {action} / {} labels | labels {} | support {} | host ms pack/update/export/fwd {:.0}/{:.0}/{:.0}/{:.0}",
            point
                .train_cosine
                .map_or_else(|| "-".to_owned(), |c| format!("{c:+.2}")),
            point.action_samples,
            point.labelled_samples,
            point
                .component_support
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("/"),
            point.packing_ms,
            point.training_host_ms,
            point.weight_export_ms,
            point.rollout_forward_ms,
        )
    } else {
        format!(
            "ev {}",
            point
                .explained_variance
                .map_or_else(|| "-".to_owned(), |ev| format!("{ev:+.2}"))
        )
    }
}

/// The update phrase of an imitation progress line: how long the update took on its own thread,
/// how long the main thread waited for it and which iteration's rollout it trained on, `upd 4.3s
/// (waited 0.0s, of #12)`; `upd -` while no update exists yet (the first iteration).
fn update_figures(point: &IterationMetricPoint) -> String {
    match point.update_of_iteration {
        Some(of) => format!(
            "upd {:.1}s (waited {:.1}s, of #{})",
            point.update_secs,
            point.update_wait_secs,
            of.saturating_add(1)
        ),
        None if point.has_update() => format!(
            "upd {:.1}s (waited {:.1}s)",
            point.update_secs, point.update_wait_secs
        ),
        None => "upd -".to_owned(),
    }
}

/// Width of the progress bar in characters.
const BAR_WIDTH: usize = 20;

/// The complete iteration record; no metrics are dropped on narrow terminals. `rewards` is
/// the recent per-step reward history for the sparkline. Plain ASCII only.
fn progress_segments(
    point: &IterationMetricPoint,
    imitation: bool,
    iter: usize,
    total: usize,
    rewards: &[f32],
) -> Vec<String> {
    let fraction = (iter as f32 / total.max(1) as f32).clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "fraction is clamped to 0..=1, so the rounded bar fill is a small non-negative count"
    )]
    let filled = (fraction * BAR_WIDTH as f32).round() as usize;
    let bar = "#".repeat(filled) + &"-".repeat(BAR_WIDTH - filled);
    let evaluations = point
        .evaluations_per_sec()
        .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.0}"));
    let mut segments = vec![
        format!("[{bar}] {:>5.1}%", fraction * 100.0),
        format!("iter {iter:>4}/{total}"),
        format!(
            "lowest episode score {}",
            point
                .best_score
                .map_or_else(|| "-".to_owned(), |b| format!("{b:.2}"))
        ),
        format!(
            "mean episode drop {}",
            point
                .mean_improvement
                .map_or_else(|| "-".to_owned(), |d| format!("{d:.2}"))
        ),
        learning_figures(point, imitation),
    ];
    if let Some(ratio) = point
        .val_improvement
        .and_then(|model| drop_ratio(model, point.val_improvement_oracle))
    {
        segments.push(format!("model/oracle {ratio:.2}"));
    }
    segments.extend([
        format!("accepted {:.1}%", point.accepted_frac * 100.0),
        rejection_figures(point),
        proposal_figures(point),
        format!("{:>6.2} steps/s", point.steps_per_sec),
        format!("{evaluations:>6} evals/s"),
    ]);
    if imitation {
        segments.push(update_figures(point));
    }
    segments.push(format!("reward [{}]", render_sparkline(rewards, 16)));
    segments
}

/// The full progress line (every segment), as a log file without a console width gets it.
#[cfg(test)]
fn progress_line(
    point: &IterationMetricPoint,
    imitation: bool,
    iter: usize,
    total: usize,
    rewards: &[f32],
) -> String {
    progress_segments(point, imitation, iter, total, rewards).join(" | ")
}

/// Records all metrics and keeps the latest completed figures beside the live runner status.
pub fn print_progress(
    console: &Console,
    runner: &TrainingRunner,
    point: &IterationMetricPoint,
    iter: usize,
    total: usize,
) {
    let rewards: Vec<f32> = runner.stats().reward_history.iter().copied().collect();
    let imitation = runner.config().algo == Algorithm::Imitation;
    let summary = progress_segments(point, imitation, iter, total, &rewards).join(" | ");
    console.iteration(&summary, completed_metrics(point, imitation));
}

fn rejection_figures(point: &IterationMetricPoint) -> String {
    format!(
        "rejected unsafe/solve/solid/girdle/manufacture/no-gain {}/{}/{}/{}/{}/{}",
        point.rej_unsafe,
        point.rej_unsolvable,
        point.rej_not_closed,
        point.rej_girdle,
        point.rej_manufacturability,
        point.rej_not_improved,
    )
}

fn proposal_figures(point: &IterationMetricPoint) -> String {
    let d = &point.diagnostics;
    format!(
        "preservation {} (in unsafe) | unchanged {} | replay added {} / cached {} | stalled restarts {}",
        d.preservation_rejected,
        d.unchanged_proposals,
        d.labels_added,
        d.labels_reused,
        d.stalled_episodes,
    )
}

/// Only completed measurements are shown; an imitation update can belong to an older rollout.
pub fn completed_metrics(point: &IterationMetricPoint, imitation: bool) -> Vec<String> {
    let learning = if imitation && point.has_update() {
        let source = point.update_of_iteration.map_or_else(
            || "completed update".to_owned(),
            |iteration| format!("completed update of #{}", iteration.saturating_add(1)),
        );
        format!(
            "{source}: loss {:.3} | cos {} | action {:.3} / {} labels",
            point.imitation_loss,
            point
                .train_cosine
                .map_or_else(|| "-".to_owned(), |value| format!("{value:+.2}")),
            point.action_loss,
            point.action_samples
        )
    } else if imitation {
        "completed update: none yet | loss - | cos - | action - / 0 labels".to_owned()
    } else {
        format!("completed PPO update: {}", learning_figures(point, false))
    };
    let mut lines = vec![
        learning,
        format!(
            "last rollout: accepted {:.1}% | {:.2} steps/s | {} evals/s",
            point.accepted_frac * 100.0,
            point.steps_per_sec,
            point
                .evaluations_per_sec()
                .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.1}"))
        ),
        proposal_figures(point),
    ];
    if imitation
        && let (Some(rows), Some(bytes), Some(budget)) = (
            point.replay_rows,
            point.replay_bytes,
            point.replay_budget_bytes,
        )
    {
        lines.push(format!(
            "replay retained {rows} rows | {:.1} / {:.1} MiB host budget",
            bytes as f64 / 1_048_576.0,
            budget as f64 / 1_048_576.0
        ));
    }
    lines
}

/// Mean rollout, label and update seconds per iteration over `history`; `None` for an empty one.
fn mean_phase_seconds(history: &[IterationMetricPoint]) -> Option<(f32, f32, f32)> {
    if history.is_empty() {
        return None;
    }
    let n = history.len() as f32;
    let mean = |seconds: fn(&IterationMetricPoint) -> f32| -> f32 {
        history.iter().map(seconds).sum::<f32>() / n
    };
    Some((
        mean(|p| p.rollout_secs),
        mean(|p| p.label_secs),
        mean(|p| p.update_secs),
    ))
}

/// Mean seconds per iteration the main thread waited for the update thread; `None` for an empty
/// history. With the update running beside the rollout this is the part of the update that
/// slowed the run down (PPO waits for the whole update).
fn mean_update_wait(history: &[IterationMetricPoint]) -> Option<f32> {
    (!history.is_empty())
        .then(|| history.iter().map(|p| p.update_wait_secs).sum::<f32>() / history.len() as f32)
}

/// Candidate evaluations per second over the whole run (the main thread's wall time per
/// iteration, see [`IterationMetricPoint::main_thread_secs`]); `None` when no evaluation or no
/// time was recorded.
fn overall_evaluations_per_sec(history: &[IterationMetricPoint]) -> Option<f32> {
    let evaluations: usize = history.iter().map(|p| p.evaluations_this_round).sum();
    let seconds: f32 = history
        .iter()
        .map(IterationMetricPoint::main_thread_secs)
        .sum();
    (evaluations > 0 && seconds > 0.0).then_some(evaluations as f32 / seconds)
}

/// Summary lines of where the time went: the workers, the mean seconds per phase and iteration,
/// and the evaluation rate.
fn print_utilisation(workers: usize, history: &[IterationMetricPoint]) {
    println!(
        "  workers                {}",
        describe_workers(workers, core_count())
    );
    if let Some((rollout, label, update)) =
        mean_phase_seconds(history).filter(|&(r, l, u)| r + l + u > 0.0)
    {
        let waited = mean_update_wait(history).unwrap_or(0.0);
        println!(
            "  seconds per iteration  rollout {rollout:.2} / label {label:.2} / update {update:.2} \
             (waited {waited:.2})"
        );
    }
    if let Some(rate) = overall_evaluations_per_sec(history) {
        println!("  evaluations per second {rate:.0}");
    }
}

/// Prints the end-of-run totals; `elapsed_secs` is the wall time of the training loop and
/// `skipped` the number of designs whose environment could not be built.
pub fn print_summary(
    runner: &TrainingRunner,
    args: &CliArgs,
    elapsed_secs: f32,
    skipped: usize,
    history: &[IterationMetricPoint],
) {
    let s = runner.stats();
    println!(
        "\nTraining complete: {} iterations, {} steps, {} episodes in {elapsed_secs:.1}s",
        args.iterations, s.total_steps, s.episodes,
    );
    if let Some(best) = &s.best_improvement {
        println!(
            "  largest score drop     {:.2} ({:.2} -> {:.2})",
            best.improvement(),
            best.base_score,
            best.final_score
        );
    }
    if let Some(best) = &s.best {
        println!("  lowest episode score   {:.2}", best.final_score);
    }
    if let Some(d) = s.mean_improvement() {
        println!("  recent mean score drop {d:.2}");
    }
    if let Some(v) = s.last_validation.as_ref().filter(|v| v.designs > 0) {
        println!(
            "  validation             score {:.2}, {}, {:.2} evals/step, brilliance {:.1}%, \
             windowing {:.1}%",
            v.mean_final_score,
            describe_drops(
                v.mean_improvement,
                v.mean_improvement_oracle,
                v.mean_improvement_sampled
            ),
            v.mean_evaluations_per_step,
            v.final_optics.brilliance,
            v.final_optics.windowing
        );
    }
    print_utilisation(runner.config().workers, history);
    if skipped > 0 {
        println!("  skipped designs        {skipped} (their environment could not be built)");
    }
}

#[cfg(test)]
#[path = "cli_report_tests.rs"]
mod tests;
