#!/usr/bin/env bash
# Runs the tests that only run on request, each by its exact name (81 tests: 12 in the pleochroma
# lib, 12 in the pleochroma-cut lib, 12 in the cut-core lib, 45 in zoning_validation). The list
# holds two kinds:
#   - the tests ignored for taking over 60 s, so the default `cargo test` skips them;
#   - the statistical tests that no other script runs (ignored as "statistical ..."): the tray
#     frames of the GPU against the CPU (renderer::gpu::estimator_check::image_comparison) and the
#     stone over a mirror with both estimators (optics::raytracer::transport::tests::
#     tray_mirror_tests). They need no feature beyond step 01's.
# The beam tests run only with scripts/run_beam_tests.sh (beam tracing is retired).
#
# Usage (from the repository root, or with --repo):
#   bash scripts/run_tests_on_request.sh [--repo DIR] [--only TEXT] [--threads N]
#                                        [--timeout DURATION]
#
#   --repo DIR          repository root (default: the current directory)
#   --only TEXT         run only the listed tests whose name contains TEXT (for example
#                       `export_thread` or `recovery::`)
#   --threads N         test threads per target (default 4: these tests are heavy, fewer at a time
#                       finish sooner than 16 competing ones)
#   --timeout DURATION  hard stop per target (default 2h; `timeout` syntax)
#
# Output: <repo>/owner-run-logs/on-request-<timestamp>/
#   NN-<target>.log  full output per cargo target
#   summary.md       exit code and duration per target
# Every target runs even when an earlier one failed.

set -u

REPO="$(pwd)"
ONLY=""
THREADS=4
TIMEOUT=2h
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --only) ONLY="$2"; shift 2 ;;
        --threads) THREADS="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
OUT_DIR="$REPO/owner-run-logs/on-request-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT_DIR"
SUMMARY="$OUT_DIR/summary.md"
{
    echo "# Tests on request, $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, so HEAD alone does not name it.
    # "-dirty" says the tree differs from HEAD; "version" is the workspace version; "sources" is
    # sources_hash (fresh_sources.sh): the hash of every file git sees (owner-run-logs left out),
    # the same on any machine.
    echo "Repo: $REPO (git: $(git describe --always --dirty 2>/dev/null || echo unknown), version: $(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1), sources: $(sources_hash)), only: '${ONLY:-all}'"
    echo "Toolchain: $(rustc -V 2>/dev/null || echo unknown), $(cargo -V 2>/dev/null || echo unknown)"
    echo
    echo "| step | exit | minutes |"
    echo "| --- | --- | --- |"
} > "$SUMMARY"

TESTS_1=(
    optics::raytracer::transport::tests::exit_splitting_tests::exit_splitting_keeps_the_mean_within_a_percent_and_does_not_add_variance
    renderer::gpu::event_wavefront::tests::ew_matches_megakernel_per_material_class
    renderer::gpu::event_wavefront::tests::ew_exit_collection_follows_the_megakernel
    renderer::gpu_backend::zoned_tests::a_zoned_opal_renders_on_the_gpu_like_on_the_cpu
    renderer::gpu_backend::zoned_tests::a_zoned_opal_without_reflections_renders_on_the_gpu_like_on_the_cpu
    renderer::gpu::frame::legacy_specialisation_tests::legacy_wavefront_reuses_only_matching_dispersion_pipelines
    renderer::gpu::frame::sorted_tests::scattering_concave_hdr_and_zoned_stones_are_twins_with_regeneration
    renderer::gpu_backend::zoned_tests::zoned_stones_render_on_the_gpu_like_on_the_cpu
    renderer::gpu::estimator_check::image_comparison::tray_shadow_tests::a_shadowed_tray_matches_the_cpu_frame
    renderer::gpu::estimator_check::image_comparison::tray_shadow_tests::a_glossy_tray_matches_the_cpu_frame
    renderer::gpu::estimator_check::image_comparison::tray_shadow_tests::a_glossy_shadowed_tray_matches_the_cpu_frame
    optics::raytracer::transport::tests::tray_mirror_tests::a_stone_over_a_mirror_changes_its_light_and_both_estimators_agree
)
TESTS_2=(
    bridge::export_thread::hybrid_export_tests::export_completes_via_hybrid_or_cpu_path
    bridge::export_thread::hybrid_export_tests::a_carried_split_runs_concurrent_batches_and_settles_on_a_valid_share
    bridge::export_thread::tests::run_export_produces_a_valid_png_for_a_tiny_scene
    bridge::export_thread::tests::run_export_with_an_hdr_environment_map_still_produces_a_valid_png
    bridge::export_thread::tests::srgb_export_is_byte_identical_regardless_of_which_save_png_path_runs
    bridge::export_thread::tests::wide_gamut_export_embeds_an_icc_profile_chunk
    bridge::render_thread::gpu_backend::concave_routing_tests::a_concave_frame_is_traced_with_its_tools_on_every_compute_target
    bridge::render_thread::gpu_backend::concave_routing_tests::a_frame_at_the_tool_limit_is_traced_with_its_tools_on_every_target
    gui::editor::identity_pins::cutting_sheet_and_diagram_bytes_are_pinned
    gui::rough_plan::run::drive_tests::a_cut_block_with_both_caps_plans_like_the_core_for_every_lane_count
    gui::rough_plan::run::drive_tests::a_mesh_rough_plans_like_the_core_for_one_and_three_lanes
    gui::rough_plan::run::drive_tests::a_mesh_rough_with_a_stone_cap_plans_like_the_core_for_one_and_three_lanes
)
TESTS_3=(
    optimize::tests::multistart::a_multi_start_run_stays_inside_its_inclusive_budget_and_reports_its_starts
    optimize::tests::multistart::the_result_does_not_depend_on_the_lane_count
    optimize::tests::shape_target::a_heavy_shape_weight_keeps_the_table_and_ratio_near_the_start
    rough_plan::colour_fit::solve::tests::a_fixed_common_gain_stays_at_one_whatever_the_photos_say
    rough_plan::colour_fit::solve::tests::an_understated_noise_model_is_caught_by_the_birge_ratio
    rough_plan::colour_fit::solve::tests::recovers_a_pale_stone_as_its_camera_metamer
    rough_plan::colour_fit::solve::tests::recovers_a_saturated_stone_as_its_camera_metamer
    rough_plan::colour_fit::solve::tests::recovers_a_tourmaline_manganese_iron_recipe_by_colour
    rough_plan::colour_fit::solve::tests::the_spread_reaches_the_farthest_feasible_point_towards_a_known_metamer
    rough_plan::colour_fit::solve::tests::the_staged_fit_keeps_the_gains_near_one_for_photos_that_need_none
    rough_plan::colour_fit::solve::tests::zoning_is_suggested_for_a_bicolour_fitted_with_one_zone_and_not_for_a_uniform_stone
    rough_plan::shaped::tests_dp::the_lane_build_reports_the_events_of_its_slices_and_cancels_across_lanes
)
TESTS_4=(
    determinism::records_and_fit_are_bitwise_identical_for_1_4_and_16_threads_and_run_to_run
    recovery::a_saturated_cube_recovers_its_face_up_colour_in_the_raw_tier
    recovery::jpeg_cube_bicolour
    recovery::jpeg_cube_pale
    recovery::jpeg_cube_saturated
    recovery::jpeg_cube_sector
    recovery::jpeg_cube_watermelon
    recovery::jpeg_immersion_bicolour
    recovery::jpeg_windows_bicolour
    recovery::main_tracer_photos_bicolour
    recovery::main_tracer_photos_pale
    recovery::main_tracer_photos_saturated
    recovery::main_tracer_photos_sector
    recovery::main_tracer_photos_watermelon
    recovery::narrow_line_bicolour_is_judged_against_its_projection
    recovery::raw_cube_bicolour
    recovery::raw_cube_pale
    recovery::raw_cube_saturated
    recovery::raw_cube_sector
    recovery::raw_cube_watermelon
    recovery::raw_frosted_bicolour
    recovery::raw_frosted_pale
    recovery::raw_frosted_saturated
    recovery::raw_frosted_sector
    recovery::raw_frosted_watermelon
    recovery::raw_holder_bicolour
    recovery::raw_holder_pale
    recovery::raw_holder_saturated
    recovery::raw_holder_sector
    recovery::raw_holder_watermelon
    recovery::raw_immersion_bicolour
    recovery::raw_immersion_pale
    recovery::raw_immersion_saturated
    recovery::raw_immersion_sector
    recovery::raw_immersion_watermelon
    recovery::raw_irregular_bicolour
    recovery::raw_irregular_pale
    recovery::raw_irregular_saturated
    recovery::raw_irregular_sector
    recovery::raw_irregular_watermelon
    recovery::raw_windows_bicolour
    recovery::raw_windows_pale
    recovery::raw_windows_saturated
    recovery::raw_windows_sector
    recovery::raw_windows_watermelon
)

# run_step NAME TEST... -- CARGO_COMMAND...: runs the command with the tests (filtered by --only)
# appended, logs it, and records exit code and minutes. A step with no test left is skipped.
run_step() {
    local name="$1"; shift
    local tests=()
    while [ "$1" != "--" ]; do
        if [ -z "$ONLY" ] || [[ "$1" == *"$ONLY"* ]]; then tests+=("$1"); fi
        shift
    done
    shift
    local log="$OUT_DIR/$name.log"
    if [ "${#tests[@]}" -eq 0 ]; then
        echo "| $name | skipped (no test matches) | 0 |" >> "$SUMMARY"
        return
    fi
    echo "== $name: ${#tests[@]} tests"
    local start=$SECONDS
    timeout "$TIMEOUT" "$@" "${tests[@]}" 2>&1 | tee "$log"
    local status=${PIPESTATUS[0]}
    local minutes=$(( (SECONDS - start + 59) / 60 ))
    echo "| $name | $status | $minutes |" >> "$SUMMARY"
}

run_step "01-pleochroma-lib" "${TESTS_1[@]}" -- cargo test --no-fail-fast -p pleochroma --lib --features gpu,path-cache-disk -- --ignored --exact --test-threads "$THREADS"
run_step "02-pleochroma-cut-lib" "${TESTS_2[@]}" -- cargo test --no-fail-fast -p pleochroma-cut --lib -- --ignored --exact --test-threads "$THREADS"
run_step "03-pleochroma-cut-core-lib" "${TESTS_3[@]}" -- cargo test --no-fail-fast -p pleochroma-cut-core --lib -- --ignored --exact --test-threads "$THREADS"
run_step "04-pleochroma-cut-core-test-zoning_validation" "${TESTS_4[@]}" -- cargo test --no-fail-fast -p pleochroma-cut-core --test zoning_validation -- --ignored --exact --test-threads "$THREADS"

echo
cat "$SUMMARY"
echo "Logs: $OUT_DIR"
# The beam rows are not in this script: run_beam_tests.sh runs them.
echo "note: the beam rows (beam tracing is retired) run only with scripts/run_beam_tests.sh (steps 03, 03g, 04b)"
