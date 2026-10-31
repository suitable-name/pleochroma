use super::*;

#[test]
fn bootstrap_and_final_at_identical_tiny_dimensions_never_mix_radiance() {
    use crate::bridge::{render_thread::local_preview::PreviewState, sample_cursor::LiveEpoch};
    let mut preview = PreviewState::default();
    let mut accum = vec![Vec3::splat(9.0)];
    let mut depth = vec![1.0];
    let mut normal = vec![Vec3::ZERO];
    let mut facet = vec![0];
    let (mut samples, mut width, mut height) = (1, 1, 1);
    let epoch = LiveEpoch::new(1, 1, 1024);
    preview.select(1, false, false, LiveComputeTarget::Both, true);
    preview.published();
    let (_, reset) = preview.select(1, false, false, LiveComputeTarget::Both, true);
    assert!(reset);
    update_accumulation_state(
        1,
        1,
        reset,
        &mut AccumulationBuffers {
            accum: &mut accum,
            first_hit_depth: &mut depth,
            first_hit_normal: &mut normal,
            first_hit_facet_id: &mut facet,
        },
        &mut samples,
        &mut width,
        &mut height,
    );
    assert_eq!(samples, 0);
    assert_eq!(accum, [Vec3::ZERO]);
    assert_eq!(
        epoch.claim_local(1),
        Some((0, 1)),
        "preview consumed no final sample range"
    );
}

#[test]
fn target_edits_preserve_accumulated_sum_and_its_original_divisor() {
    let mut accum = vec![Vec3::splat(256.0)];
    let mut depth = vec![1.0];
    let mut normal = vec![Vec3::ZERO];
    let mut facet = vec![0];
    let (mut samples, mut width, mut height) = (128, 1, 1);
    for target in [1024, 8] {
        update_accumulation_state(
            1,
            1,
            false,
            &mut AccumulationBuffers {
                accum: &mut accum,
                first_hit_depth: &mut depth,
                first_hit_normal: &mut normal,
                first_hit_facet_id: &mut facet,
            },
            &mut samples,
            &mut width,
            &mut height,
        );
        assert_eq!(samples, 128);
        assert_eq!(accum[0] / 128.0, Vec3::splat(2.0));
        assert_eq!(samples >= target, target == 8);
    }
}

// ---- pacing_sleep: only the unused remainder of the frame interval ---------------

#[test]
fn a_paced_iteration_sleeps_only_the_unused_remainder() {
    let frame = Duration::from_millis(16);
    assert_eq!(
        pacing_sleep(true, Duration::from_millis(5), frame),
        Some(Duration::from_millis(11))
    );
}

#[test]
fn a_paced_iteration_that_ran_long_does_not_sleep() {
    let frame = Duration::from_millis(16);
    assert_eq!(pacing_sleep(true, Duration::from_millis(16), frame), None);
    assert_eq!(pacing_sleep(true, Duration::from_millis(40), frame), None);
}

#[test]
fn an_unpaced_iteration_never_sleeps() {
    assert_eq!(
        pacing_sleep(false, Duration::ZERO, Duration::from_millis(16)),
        None
    );
}

// ---- tracing_suspended: the four independent suspend flags ----------------------

#[test]
fn tracing_runs_only_when_all_four_flags_allow_it() {
    assert!(
        !SuspensionFlags {
            paused: false,
            tab_visible: true,
            remote_suspends: false,
            export_active: false,
            material_unresolved: false
        }
        .tracing_suspended()
    );
}

#[test]
fn each_flag_alone_suspends_tracing() {
    assert!(
        SuspensionFlags {
            paused: true,
            tab_visible: true,
            remote_suspends: false,
            export_active: false,
            material_unresolved: false
        }
        .tracing_suspended(),
        "paused"
    );
    assert!(
        SuspensionFlags {
            paused: false,
            tab_visible: false,
            remote_suspends: false,
            export_active: false,
            material_unresolved: false
        }
        .tracing_suspended(),
        "!tab_visible"
    );
    assert!(
        SuspensionFlags {
            paused: false,
            tab_visible: true,
            remote_suspends: true,
            export_active: false,
            material_unresolved: false
        }
        .tracing_suspended(),
        "remote_suspends"
    );
    assert!(
        SuspensionFlags {
            paused: false,
            tab_visible: true,
            remote_suspends: false,
            export_active: true,
            material_unresolved: false
        }
        .tracing_suspended(),
        "export_active"
    );
}

#[test]
fn every_flag_combination_still_suspends_if_any_one_is_set() {
    for paused in [false, true] {
        for tab_visible in [false, true] {
            for remote_suspends in [false, true] {
                for export_active in [false, true] {
                    let expected = paused || !tab_visible || remote_suspends || export_active;
                    assert_eq!(
                        SuspensionFlags {
                            paused,
                            tab_visible,
                            remote_suspends,
                            export_active,
                            material_unresolved: false,
                        }
                        .tracing_suspended(),
                        expected,
                        "paused={paused} tab_visible={tab_visible} \
                             remote_suspends={remote_suspends} export_active={export_active}"
                    );
                }
            }
        }
    }
}

// ---- metrics_suspended: narrower than tracing_suspended ----

#[test]
fn an_invisible_tab_alone_does_not_suspend_metrics() {
    assert!(
        !SuspensionFlags {
            paused: false,
            tab_visible: false,
            remote_suspends: false,
            export_active: false,
            material_unresolved: false,
        }
        .metrics_suspended(),
        "an invisible 3D tab must not freeze the HUD/tilt-dialog metrics -- \
             only tracing_suspended (the full trace) should see !tab_visible"
    );
}

#[test]
fn every_hard_suspend_reason_still_suspends_metrics() {
    assert!(
        SuspensionFlags {
            paused: true,
            tab_visible: true,
            remote_suspends: false,
            export_active: false,
            material_unresolved: false,
        }
        .metrics_suspended(),
        "paused"
    );
    assert!(
        SuspensionFlags {
            paused: false,
            tab_visible: true,
            remote_suspends: true,
            export_active: false,
            material_unresolved: false,
        }
        .metrics_suspended(),
        "remote_suspends"
    );
    assert!(
        SuspensionFlags {
            paused: false,
            tab_visible: true,
            remote_suspends: false,
            export_active: true,
            material_unresolved: false,
        }
        .metrics_suspended(),
        "export_active"
    );
}

#[test]
fn metrics_suspended_is_never_stricter_than_tracing_suspended() {
    // metrics_suspended must be a subset of tracing_suspended's reasons: it
    // would make no sense for metrics to be blocked while full tracing (a
    // strictly more expensive computation) is allowed to run.
    for paused in [false, true] {
        for tab_visible in [false, true] {
            for remote_suspends in [false, true] {
                for export_active in [false, true] {
                    let flags = SuspensionFlags {
                        paused,
                        tab_visible,
                        remote_suspends,
                        export_active,
                        material_unresolved: false,
                    };
                    if flags.metrics_suspended() {
                        assert!(
                            flags.tracing_suspended(),
                            "metrics_suspended implies tracing_suspended: {flags:?}"
                        );
                    }
                }
            }
        }
    }
}

// ---- remote_suspends_local / should_combine_remote: the mode-dependent split ----

#[test]
fn remote_only_is_the_sole_mode_where_remote_activity_suspends_local_tracing() {
    for mode in [
        LiveComputeTarget::LocalOnly,
        LiveComputeTarget::RemoteOnly,
        LiveComputeTarget::Both,
    ] {
        assert!(
            !remote_suspends_local(false, mode),
            "remote_active == false must never suspend, regardless of mode"
        );
    }
    assert!(
        !remote_suspends_local(true, LiveComputeTarget::LocalOnly),
        "LocalOnly never dispatches remote, but even if remote_active were \
             somehow true, LocalOnly must not suspend local tracing"
    );
    assert!(
        remote_suspends_local(true, LiveComputeTarget::RemoteOnly),
        "RemoteOnly must reproduce the pre-existing suspend-while-remote-active \
             behaviour exactly"
    );
    assert!(
        !remote_suspends_local(true, LiveComputeTarget::Both),
        "Both must never suspend local tracing -- it keeps contributing, it never \
             yields the buffer to remote"
    );
}

#[test]
fn only_both_mode_combines_and_only_while_remote_is_still_active() {
    for mode in [
        LiveComputeTarget::LocalOnly,
        LiveComputeTarget::RemoteOnly,
        LiveComputeTarget::Both,
    ] {
        assert!(
            !should_combine_remote(false, mode),
            "remote_active == false must never combine, regardless of mode"
        );
    }
    assert!(!should_combine_remote(true, LiveComputeTarget::LocalOnly));
    assert!(
        !should_combine_remote(true, LiveComputeTarget::RemoteOnly),
        "RemoteOnly's local tracing is suspended whenever this could matter -- it \
             must never itself decide to combine"
    );
    assert!(should_combine_remote(true, LiveComputeTarget::Both));
}

#[test]
fn combining_persists_past_a_successful_remote_completion() {
    // `remote_active` deliberately stays `true` past `RemoteUpdate::Done` -- for
    // `Both`, this lets local's continued tracing keep ADDING to remote's finished
    // contribution.
    assert!(should_combine_remote(true, LiveComputeTarget::Both));
}

/// Verify requirement: "a settled combined render is not overwritten by either
/// engine" -- local's growing sample count only ever ADDS to what
/// the live epoch's remote sum already holds, so the combined image can only converge
/// further, never regress to a from-scratch restart.
#[test]
fn a_settled_combined_render_keeps_combining_rather_than_being_overwritten() {
    for _quiet_frame in 0..5 {
        let resolved = resolve_remote_ownership(false, true, true);
        assert!(resolved);
        assert!(should_combine_remote(resolved, LiveComputeTarget::Both));
    }
}

/// Verify requirement: "a scene change during a combined render restarts
/// accumulation correctly" -- must stop folding a now-stale-scene remote
/// contribution into the display, the same release `update_accumulation_state`'s
/// `dirty` handling already resets `accum_buffer` for.
#[test]
fn a_scene_change_during_a_combined_render_stops_combining_a_now_stale_contribution() {
    let resolved = resolve_remote_ownership(true, true, true);
    assert!(
        !resolved,
        "a genuine scene invalidation must release ownership even mid-combine"
    );
    assert!(
        !should_combine_remote(resolved, LiveComputeTarget::Both),
        "combining must stop the instant ownership is released, so local's freshly \
             reset accum_buffer (see update_accumulation_state's own dirty handling) is \
             never summed with a now-stale-scene remote contribution"
    );
}

// ---- resolve_remote_ownership: every dirty/remote_active transition it must
// resolve correctly (hand-off start, ownership release, steady states) -- see its
// own doc comment for the full reasoning these tests hold it to. ----------------

#[test]
fn nothing_active_and_no_dirty_stays_inactive() {
    assert!(!resolve_remote_ownership(false, false, false));
}

#[test]
fn the_handoffs_own_settling_entry_does_not_release_ownership() {
    // DiscardLocalPreview sets `dirty` and `remote_active` together in one locked
    // mutation, starting from `remote_active == false` -- the frame that first
    // observes it has `dirty` and `remote_active` freshly true, `false` one frame ago.
    assert!(
        resolve_remote_ownership(true, true, false),
        "the hand-off starting must not immediately cancel itself"
    );
}

#[test]
fn a_completed_remote_render_is_not_overwritten_by_local_tracing() {
    // Steady state after `RemoteUpdate::Done`: `remote_active` stays `true`, and
    // every quiet frame (`dirty == false`) must leave it alone -- the regression
    // test for local tracing restarting and overwriting a finished remote image.
    assert!(resolve_remote_ownership(false, true, true));
}

#[test]
fn a_scene_change_after_a_completed_remote_render_resumes_local_tracing() {
    // A `gui::*` callback sets `ctx.dirty = true` for a reason unrelated to the
    // handoff machine; ownership must be released so local tracing resumes.
    assert!(!resolve_remote_ownership(true, true, true));
}

#[test]
fn a_dirty_frame_while_already_inactive_stays_inactive() {
    assert!(!resolve_remote_ownership(true, false, false));
    assert!(!resolve_remote_ownership(true, false, true));
}

#[test]
fn remote_rendering_survives_quiet_frames_with_no_dirty() {
    assert!(resolve_remote_ownership(false, true, true));
    assert!(resolve_remote_ownership(false, true, false));
}

// ---- update_accumulation_state: suspension must never discard samples -----------

#[test]
fn a_quiet_frame_with_unchanged_dimensions_preserves_accumulated_samples() {
    // Models a suspended frame (paused/tab-hidden/remote_active/export_active):
    // `dirty == false`, same width/height. Neither branch may fire.
    let mut accum = vec![Vec3::new(1.0, 2.0, 3.0); 4];
    let mut first_hit_depth = vec![0.5; 4];
    let mut first_hit_normal = vec![Vec3::X; 4];
    let mut first_hit_facet_id = vec![7; 4];
    let mut accum_samples = 42;
    let mut last_width = 2;
    let mut last_height = 2;

    update_accumulation_state(
        2,
        2,
        false,
        &mut AccumulationBuffers {
            accum: &mut accum,
            first_hit_depth: &mut first_hit_depth,
            first_hit_normal: &mut first_hit_normal,
            first_hit_facet_id: &mut first_hit_facet_id,
        },
        &mut accum_samples,
        &mut last_width,
        &mut last_height,
    );

    assert_eq!(
        accum_samples, 42,
        "a suspended frame must not reset progress"
    );
    assert_eq!(
        accum,
        vec![Vec3::new(1.0, 2.0, 3.0); 4],
        "the running sum must survive"
    );
}

#[test]
fn a_dirty_frame_still_resets_accumulation_regardless_of_remote_ownership() {
    // `update_accumulation_state` only sees `dirty`/`width`/`height`, not
    // `remote_active`, by design: the reset and ownership-release decisions are
    // independent, both computed from the same `dirty` flag.
    let mut accum = vec![Vec3::new(1.0, 2.0, 3.0); 4];
    let mut first_hit_depth = vec![0.5; 4];
    let mut first_hit_normal = vec![Vec3::X; 4];
    let mut first_hit_facet_id = vec![7; 4];
    let mut accum_samples = 42;
    let mut last_width = 2;
    let mut last_height = 2;

    update_accumulation_state(
        2,
        2,
        true,
        &mut AccumulationBuffers {
            accum: &mut accum,
            first_hit_depth: &mut first_hit_depth,
            first_hit_normal: &mut first_hit_normal,
            first_hit_facet_id: &mut first_hit_facet_id,
        },
        &mut accum_samples,
        &mut last_width,
        &mut last_height,
    );

    assert_eq!(accum_samples, 0);
    assert_eq!(accum, vec![Vec3::ZERO; 4]);
}

// ---- update_accumulation_state: capacity reuse across resizes --------------------

#[test]
fn resizing_keeps_capacity_and_reinitialises_every_element() {
    let mut accum = Vec::new();
    let mut depth = Vec::new();
    let mut normal = Vec::new();
    let mut facet = Vec::new();
    let (mut samples, mut width, mut height) = (7, 0, 0);
    {
        let mut run = |w: u32, h: u32, dirty: bool, samples: &mut u32| {
            update_accumulation_state(
                w,
                h,
                dirty,
                &mut AccumulationBuffers {
                    accum: &mut accum,
                    first_hit_depth: &mut depth,
                    first_hit_normal: &mut normal,
                    first_hit_facet_id: &mut facet,
                },
                samples,
                &mut width,
                &mut height,
            );
        };

        run(8, 8, false, &mut samples);
        assert_eq!(samples, 0);
        run(4, 4, false, &mut samples);
        run(8, 8, false, &mut samples);
    }

    assert!(accum.capacity() >= 64);
    assert!(depth.capacity() >= 64);
    assert!(normal.capacity() >= 64);
    assert!(facet.capacity() >= 64);
    assert_eq!(accum, vec![Vec3::ZERO; 64]);
    assert_eq!(depth, vec![1.0e6_f32; 64]);
    assert_eq!(normal, vec![Vec3::ZERO; 64]);
    assert_eq!(facet, vec![-1_i32; 64]);
}

#[test]
fn shrinking_resize_does_not_reallocate_and_regrowth_reuses_the_allocation() {
    let mut accum = Vec::new();
    let mut depth = Vec::new();
    let mut normal = Vec::new();
    let mut facet = Vec::new();
    let (mut samples, mut width, mut height) = (0, 0, 0);
    let mut call = |w: u32, h: u32, accum: &mut Vec<Vec3>| {
        update_accumulation_state(
            w,
            h,
            false,
            &mut AccumulationBuffers {
                accum,
                first_hit_depth: &mut depth,
                first_hit_normal: &mut normal,
                first_hit_facet_id: &mut facet,
            },
            &mut samples,
            &mut width,
            &mut height,
        );
    };

    call(8, 8, &mut accum);
    let full_ptr = accum.as_ptr();
    call(4, 4, &mut accum);
    assert!(
        accum.capacity() >= 64,
        "a preview-sized frame must keep the full frame's capacity"
    );
    assert_eq!(accum.as_ptr(), full_ptr);
    call(8, 8, &mut accum);
    assert_eq!(accum.as_ptr(), full_ptr, "regrowth reuses the allocation");
}

#[test]
fn dirty_with_unchanged_dimensions_still_zeroes_and_resets_samples() {
    let mut accum = vec![Vec3::splat(3.0); 4];
    let mut depth = vec![0.5; 4];
    let mut normal = vec![Vec3::X; 4];
    let mut facet = vec![7; 4];
    let (mut samples, mut width, mut height) = (9, 2, 2);
    update_accumulation_state(
        2,
        2,
        true,
        &mut AccumulationBuffers {
            accum: &mut accum,
            first_hit_depth: &mut depth,
            first_hit_normal: &mut normal,
            first_hit_facet_id: &mut facet,
        },
        &mut samples,
        &mut width,
        &mut height,
    );
    assert_eq!(samples, 0);
    assert_eq!(accum, vec![Vec3::ZERO; 4]);
    // Guides are overwritten by the tracer every pass; a plain dirty reset leaves them.
    assert_eq!(depth, vec![0.5; 4]);
}

#[test]
fn dirty_resize_zeroes_accum_and_resets_samples_once() {
    let mut accum = vec![Vec3::splat(3.0); 4];
    let mut depth = vec![0.5; 4];
    let mut normal = vec![Vec3::X; 4];
    let mut facet = vec![7; 4];
    let (mut samples, mut width, mut height) = (9, 2, 2);
    update_accumulation_state(
        3,
        3,
        true,
        &mut AccumulationBuffers {
            accum: &mut accum,
            first_hit_depth: &mut depth,
            first_hit_normal: &mut normal,
            first_hit_facet_id: &mut facet,
        },
        &mut samples,
        &mut width,
        &mut height,
    );
    assert_eq!(samples, 0);
    assert_eq!((width, height), (3, 3));
    assert_eq!(accum, vec![Vec3::ZERO; 9]);
    assert_eq!(depth, vec![1.0e6_f32; 9]);
    assert_eq!(facet, vec![-1_i32; 9]);
}
