# pleochroma-cut — Settings File

The on-disk settings format: location, persistence guarantees, render quality,
and the remote endpoint. For everything else see [the README](../README.md).

## Settings File

Location, resolved by `settings::store::default_settings_path()`:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\pleochroma-cut\settings.toml` |
| macOS | `~/Library/Application Support/pleochroma-cut/settings.toml` |
| Linux/Unix | `$XDG_CONFIG_HOME/pleochroma-cut/settings.toml`, else `~/.config/pleochroma-cut/settings.toml` |
| (no env var found) | `./pleochroma-cut/settings.toml`, relative to the current directory |

Hand-rolled per-platform resolution (no `directories`/`dirs` crate), chosen to
avoid needing write access to the install directory (e.g. under `Program Files`
on Windows). Format is TOML, written via `toml::to_string_pretty`, and writes
are crash-safe: written to a `.toml.tmp` sibling first, then atomically renamed
over the real file. **Loading is deliberately infallible** — a missing or
unreadable settings file just logs a warning and falls back to defaults, and a
broken settings file can never block startup, since every field has a
`#[serde(default)]` so an old/partial file loads fine with the missing fields
defaulted. A **corrupt** (unparseable TOML) file is not silently discarded: it
is renamed aside first, to `settings.toml.corrupt-<unix-seconds>` next to it,
*before* falling back to defaults — the next save then writes a fresh
`settings.toml` rather than overwriting the corrupt one, so the broken file (and
whatever was in it) stays recoverable on disk instead of being lost outright. A
failed rename (e.g. no write permission on the directory) is logged and startup
continues with defaults anyway; the corrupt file is simply left at its original
path in that case.

**Legacy migration.** The first time this app runs with no settings file of its
own yet, it copies (never moves) a settings file from the config directory of the
old viewer app, `diagram-gui`, which `pleochroma-cut` superseded, so a machine
that already had that app configured doesn't start completely blank.
This only ever fires into a genuinely absent destination and never overwrites
an existing `pleochroma-cut` settings file; a failed copy is logged and falls
through to defaults, same as any other load failure.

Example of what the file looks like:

```toml
[settings]
target_samples = 256
max_bounces = 12
exposure = 1.0
light_yaw_deg = 48.0
light_pitch_deg = 72.0
lighting_rig = "Light tent + black cards"
camera_yaw = 0.35
camera_pitch = 1.15
camera_distance = 2.4
selected_material = "Diamond"
denoise_enabled = true
contribute_to_final_picture = true
payload_encoding = "auto"
remote_batch_lanes = 4
remote_batch_lanes_user_set = false
remote_preview_batch_size = 10
surface_glare = 1.0
import_preview_choice = "ask"

[settings.remote]
export_transfer = "FullData"
live_transfer = "FullData"

[settings.remote.connection]
name = "Coordinator"
address = "10.0.0.5:7878"
cert_dir = "C:/Users/me/pleochroma-certs"
transfer_mode = "LiveProgressive"
cadence_ms = 500
preview_scale = "Quarter"

[[presets]]
name = "Studio Softbox"
built_in = true
light_yaw_deg = 48.0
light_pitch_deg = 54.0
exposure = 1.0
lighting_rig = "Gem Studio Ring Lights"
camera_distance = 2.4
```

### Target Samples

Render quality is one setting, `target_samples`: how many samples per pixel the
progressive accumulation converges to before it stops. Default 256.

The dialog's slider drags an **exponent**, not the count — `2^3 = 8` up to
`2^10 = 1024` (`gui::render::sample_scale`). Noise falls as `1/sqrt(N)`, so a linear
8..1024 control would spend roughly 97% of its travel above 32 spp, where each
step barely changes anything visible. A `target_samples` value that is not an
exact power of two still loads: it resolves to the largest power of two not
exceeding it, so a hand-edited file can never fail to open.

An older settings file may still carry a `quality_preset = "High / Quality"`
string from a retired four-tier preset selector. Nothing rejects it —
there is no `#[serde(deny_unknown_fields)]`, so TOML drops the unknown key and
`target_samples` takes its default.

`target_samples` is also the one target for a live image rendered with a remote:
local and remote samples count toward it together. The separate
`remote_render_samples` key older files carry is ignored on load and dropped on the
next save.

### Surface Glare

`surface_glare` (`0.0..=1.0`, default `1.0`) scales the white mirror image of the
light on a polished surface -- the cross-polarised look; the Lighting & Environment tab's "Surface Glare"
slider drags it as a percent in steps of 5. `1.0` is the unscaled render, `0.0` keeps
only light that entered the stone. It applies to the built-in lighting presets, never
to an HDR map, and follows into the live view, remote workers and exports (it rides
`SceneState::surface_glare`, protocol v18). The metrics, tilt curves and catalogue
previews never read it. The key is optional (a file without it loads `1.0`) and a
hand-edited value outside the range loads as the nearest valid one (NaN as `1.0`).
It is part of every saved lighting (`LightingPreset::surface_glare`, serde default `1.0`
for presets saved by an older build) and of a design's stored lighting.

### Stone Shadow

`stone_shadow` is a boolean, default `false`. The "Stone Shadow" switch in the
Lighting & Environment tab makes the stone cast a shadow and colored caustics onto
the tray touching its culet. It needs a studio light with a backdrop; it dims with
an HDR map, As Lit or "Contrast view (ASET-style)". It persists globally, in saved
lighting, and in each design's stored lighting. Older settings and saved rows load it
as off. Live viewing builds a Preview map in the background and restarts accumulation
when it arrives. Camera moves reuse the map; light changes rebuild it. Exports, jobs,
final images, batches and the passes of an adaptive export build Final quality before
tracing. Remote workers compute their own maps, Preview for a live viewport request
and Final for the rest (`SceneState::stone_shadow`, protocol 40). Tilt views and
videos omit it.

### Tray Reflection

`tray_reflection` is a number from `0.0` to `1.0`, default `0.0` (off). The "Tray Reflection"
slider in the Lighting & Environment tab, under "Stone Shadow", makes the tray under the stone a
glossy surface; the value is its head-on reflectance (the slider shows 0 to 100 %, "Off" at 0).
About 4 % looks like black acrylic or glass, which with the Black backdrop gives the classic
jewelry-photo look; 100 % is a mirror. Glancing views reflect more than head-on ones. What the
surface does not reflect is the ordinary tray, including the Stone Shadow when that is on, and
the stone appears mirrored in it. It has the Stone Shadow's scope: it needs a studio light with a
backdrop, and the row dims with an HDR map, As Lit or "Contrast view (ASET-style)", showing the
Stone Shadow row's notes ("(not used with an HDR map)", "(needs a backdrop)", "(not used with
this light)"). It persists globally, in saved
lighting, and in each design's stored lighting; older settings and saved rows load it as off.
The path cache does not serve it, and neither does beam tracing (in a build with the `beam`
feature); notes say so. Spectral GPU rendering leaves the render to the processor when the card
cannot run it; Ray-Traced GPU rendering always runs it. Tilt views and videos omit it ("Tilt views don't show
the tray reflection."). Remote workers build the mirror themselves
(`SceneState::tray_reflection`, protocol 41).

### Head Shadow

`head_shadow_deg` (`0.0..=30.0`, default `16.0`) is the angular radius, in degrees, of
the viewer's head shadow on the lit lighting rigs (light tent and its variants, grading
tray, daylight sky); `0.0` turns it off. The Lighting & Environment tab's "Head Shadow" slider drags it in whole
degrees. The Studio rigs and an HDR map ignore it. It follows into the
live view, remote workers and exports (it rides `SceneState::head_shadow_deg`, protocol
v22, and `RenderContext::head_shadow_deg` into the scene identity, so a change restarts
accumulation). The key is optional (a file without it loads `16.0`) and a hand-edited
value outside the range loads as the nearest valid one (NaN as `16.0`). The on-screen
brilliance, windowing and extinction figures follow the slider under the lit presets;
the Optimize and Retarget searches always score with the default 16 degrees. It is part
of every saved lighting and of a design's stored lighting (`head_shadow_deg`, serde
default `16.0` for rows and presets saved before it existed).

### Bounce Cap

`max_bounces` is independent of the sample count. Default 12; the dialog offers
4 / 8 / 12 / 24 / 64 / 128. Measured with `crates/pleochroma/examples/bounce_cost.rs`:
a cap of 1024 costs only 1.3-1.4x the wall time of 12 on CPU (paths are short-tailed,
median 4 bounces, and only 0.03% reach 128), while the hardest material converges to
95.6% at 24, 99.5% at 64 and 99.99% at 128. Nothing above 128 is offered because
nothing measurably changes past it.

A persisted value that is no longer a rung (16, say) is still honoured exactly
as written for rendering; only the highlighted pill snaps to the nearest rung.

Render resolution is its own setting (`render_width`/`render_height`), not part
of either control.

### Parameters Dock, Output Defaults and the Stone Migration

Keys written by the Parameters dock (Stone / Lighting & Environment / Render & Output):

| Key | Values | Default |
|---|---|---|
| `parameters_dock_open` | `true` / `false` | `false` (a file without the key loads folded) |
| `parameters_dock_tab` | `0` Stone, `1` Lighting & Environment, `2` Render & Output | `0` |
| `migrated_stone_effects` | `true` / `false` | `false` for a file without the key, `true` on a brand-new install |
| `[settings.update_check]` | `enabled` (`true` / `false`; absent = not asked yet), `last_check_unix` (seconds), `skipped_version` (`"0.7.3"`) | not asked, never checked, none skipped |
| `[settings.output.still]` | `size_preset`, `custom_width`, `custom_height`, `sample_exponent`, `bounces`, `color_space_index`, `compute`, `transfer`, `adaptive_sampling`, `target_noise_index`, `path_cache`, `keep_path_cache`, `beam` (Beam Tracing (Faceted Stones)), `keep_beams` (Keep Beams for Re-Renders) | the still export's defaults (the boxes are `false`). Beam tracing is retired: every build reads and writes `beam` and `keep_beams` unchanged, but only a build with the `beam` feature shows or uses them. Adaptive sampling is an optional build feature (`adaptive-sampling`): every build reads and writes `adaptive_sampling` and `target_noise_index` unchanged, but only a build with the feature shows or uses them |
| `[settings.output.tilt_video]` | `resolution_preset`, `custom_width`, `custom_height`, `sample_exponent`, `bounces`, `color_space_index`, `transfer`, `filename_template`, `start_angle_deg` / `end_angle_deg` (-90..90), `step_hundredths_deg` (1..500, so 0.01°..5°), `fps_index` (`0` 24, `1` 30, `2` 60), `axes` (chosen axes `0`..`3`; empty = the chart's axis), `quarter_turn`, `show_values`, `metric_brilliance`, `metric_windowing`, `metric_extinction`, `metric_tilt_brilliance`, `metric_angle`, `keep_frames`, `adaptive_sampling` (same rule as in `[settings.output.still]`), `target_noise_index`, `path_cache`, `keep_video_cache` (Keep Path Cache for This Video; in a build with the `beam` feature it keeps the beams too), `beam` (Beam Tracing (Faceted Stones)) | the tilt video's defaults (the boxes are `false`). Every build reads and writes `beam` unchanged; only a build with the retired `beam` feature shows or uses it |

**Output defaults.** `[settings.output]` is what the Export and Tilt Video dialogs open
with. Both tables and every field are optional (a file without them loads the defaults),
and the values are limited to what their controls allow when read. Starting an export or
a tilt video, adding one to the queue, and closing the Export or Tilt Performance dialog
write the values back, so the dialogs open as you left them. A
`transfer` of nothing follows the remote endpoint's `export_transfer`. The tilt video's
compute pill stays in `tilt_video_compute_target`.

**Stone effects and width (migration).** The stone's width and its four material effects
(inclusion haze, crystal axis, frosted girdle, edge rounding) are stored in each design
(`girdle_diameter_mm` and the optional `[render_effects]` table of the `.psdd`
file). The old app-level keys `stone_width_mm`, `inclusion_sigma_s`,
`c_axis_override_enabled`, `c_axis_tilt_deg`, `c_axis_azimuth_deg`, `girdle_frosted` and
`edge_rounding_radius` stay in the file and are not read for rendering. If any of
them is off its default and `migrated_stone_effects` is `false`, they are applied once,
as one undo step, to the first design opened that has neither a `[render_effects]` table
nor a girdle diameter; then `migrated_stone_effects` becomes `true` and they are ignored.

**Saved lighting.** `[[presets]]` entries also carry `surface_glare`, `backdrop`,
`stone_shadow`, `tray_reflection` and `head_shadow_deg` (serde defaults `1.0`, `"White"`, `false`, `0.0`, `16.0` for presets saved by an older
build), so applying a saved lighting sets the whole look. They also carry the map rotation
(`env_map_yaw_deg`, `env_map_follows_camera`, defaults `0` and `false`), which reaches fan-out
exports, render jobs (format 6 for a turned HDR scene), remote renders and tilt videos the way
the live view uses it. A newly saved lighting is
named "<rig> - <date>" by default.

### Interface Preferences

Edit > Preferences... (Ctrl+Comma) writes these top-level keys; the
[manual chapter](manual/17-preferences-and-accessibility.md) explains what each one does.

| Key | Values | Default |
|---|---|---|
| `ui_mode` | `"simple"` or `"advanced"` | `"advanced"` for a file without the key; `"simple"` on a brand-new install |
| `first_run_tour_done` | `true` / `false` | `true` for a file without the key; `false` on a brand-new install |
| `tutorials_completed` | list of tutorial ids | empty |
| `ui_scale_percent` | `0` (Automatic) or one of 75, 90, 100, 110, 125, 150, 175, 200 | `0` |
| `high_contrast` | `true` / `false` | `false` |
| `large_handles` | `true` / `false` | `false` |
| `adaptive_viewport` | `true` / `false`: Adaptive Sampling in the Viewport; every build reads and writes it, only a build with the optional `adaptive-sampling` feature shows or uses it | `false` |
| `path_cache_viewport` | `true` / `false`: Speed Up Still Views With a Path Cache; read and written as before, but not used while the app renders with exit collection (the box is greyed out with a note) | `false` |
| `instant_lighting` | `true` / `false`: Instant Lighting Changes in the Ray-Traced View | `false` |
| `beam_viewport` | `true` / `false`: Beam Tracing in Still Views; every build reads and writes it, only a build with the retired `beam` feature shows or uses it, and the Path-Traced view ignores it under exit collection | `false` |
| `beam_instant_lighting` | `true` / `false`: Instant Lighting Changes With Beams; counts only while `beam_viewport` is `true`; same rule as `beam_viewport` | `false` |
| `path_cache_disk_limit_gb` | The most disk space kept path caches may use, in GB (1 to 100000) | `50` |
| `manipulate_snap_off` | `true` / `false` | `false` |
| `slice_symmetric` | `true` / `false` | `true` |
| `show_crystal_axis` | `true` / `false` | `false` |

**New install versus existing install.** Only an *absent* settings file is a brand-new
install: it starts in the Simple interface with the welcome tour still to come. A file
that exists but lacks these keys (an install from before the switch), and a file that
could not be read, belong to someone who already uses the app, so they load `"advanced"`
with the tour marked done -- nobody loses controls or gets a tour they did not ask for.
An unfamiliar `ui_mode` word loads as `"advanced"`.

**UI scale.** `ui_scale_percent` is read once, at the very start of the program
(`settings::store::peek_ui_scale_percent`), and handed to the windowing layer as
`SLINT_SCALE_FACTOR` (for example `1.25`) before the first window exists, so a change
needs a restart. If you set `SLINT_SCALE_FACTOR` yourself, your value wins. A value that is
not offered (a hand-edited `133`, say) loads as `0`, Automatic.

**Snap and Slice.** `manipulate_snap_off` is the Solid viewport's Snap pill and
`slice_symmetric` is the Slice tool's Symmetric pill; both are remembered. Whether the
Slice tool itself is on (`slice_mode`) is deliberately not saved: it changes what a
left-drag does, so every session starts with it off.

**Crystal Axis display.** `show_crystal_axis` is the Solid viewport's Crystal Axis pill, which
draws the stone's crystal axis over it. It is remembered. The app also switches the pill on
for the rest of a session, without saving it, the first time you set an axis or open a
planned stone that came with one; switching it off yourself keeps it off. Chapter 25 of the
manual describes the axis.

**Faceting Machine.** `faceting_machine` is a table with `arm_offset_deg`, `lap_radius_mm`, `mast_distance_mm`, `sweep_inner_mm` and `sweep_outer_mm`: how the cutter's machine moves the stone over the lap, which gives the scratch direction of a polished facet (Chapter 26 of the manual). It is a per-user preference and is never saved in a design. It is written only when it differs from the plain machine (arm at the lap centre, a 100 mm lap, a sweep from 40 to 80 mm).

### Other Top-Level Keys

The remaining keys of `AppSettings` (`settings/model/app_settings.rs`). Every one is
optional: a file without it loads the default shown.

| Key | Values | Default |
|---|---|---|
| `render_width`, `render_height` | live render size in pixels (the dialog offers 640x480, 800x600, 1280x720, 1920x1080; a hand-edited size still loads) | `800`, `600` |
| `backdrop` | `"AsLit"`, `"Grey"`, `"White"`, `"Black"`: the lit tray under the stone, seen around and through it (As Lit = the lighting's own ground) | `"White"` |
| `stone_shadow` | `true` / `false`: the stone's shadow and colored caustics on a studio backdrop | `false` |
| `tray_reflection` | `0.0` to `1.0`: the head-on reflectance of the tray under the stone (about `0.04` = black acrylic, `1.0` = mirror); `0.0` is off | `0.0` |
| `live_compute_target` | `"LocalOnly"`, `"RemoteOnly"`, `"Both"` (does nothing without a configured remote) | `"Both"` |
| `local_compute_target` | `"Cpu"`, `"CpuGpu"`, `"Gpu"` | `"CpuGpu"` |
| `local_preview_scale` | `"Off"`, `"Half"`, `"Quarter"`: resolution while the camera moves, full size once it settles | `"Off"` |
| `tilt_video_compute_target` | `"Local"`, `"Remote"`, `"Both"` (the tilt video's Compute pill) | `"Both"` |
| `plan_time_limit_secs` | seconds; the Rough planner's scan plan time limit, `0` is no limit | `120` |
| `env_map_path` | path of the last loaded Radiance `.hdr` map; empty means the built-in lighting | empty |
| `env_map_yaw_deg` | `0` to `360` degrees: how far the loaded HDR map is turned around the stone (the "Map Rotation" slider, or a right-drag in the viewport); a hand-edited value wraps into the range, NaN loads `0`; the key is left out when it is `0` | `0` |
| `env_map_follows_camera` | `true` / `false`: the "Turns with the Camera" switch. On, the map's yaw gets the camera yaw added, so the map stays fixed relative to the view and the stone orbits under it; off, the map is fixed in the room. The key is left out when it is `false` | `false` |
| `preview_size`, `preview_spp` | square size in pixels and samples per pixel of the catalogue front/top thumbnails | `160`, `256` |
| `export_directory` | folder every high-resolution export writes into; empty means not chosen yet (the next export asks) | empty |
| `last_import_directory` | folder the `.asc` pickers open in; empty leaves the OS default | empty |
| `export_filename_template` | export filename template | `gem_export_{material}_{width}x{height}_{spp}spp_{timestamp}` |
| `library_panel_collapsed` | `true` / `false`: the left library panel folded to a rail | `false` |
| `solid_view_mode` | `0` Solid, `1` Path-Traced, `2` Solid + Path-Traced, `3` Diagram, `4` Ray-Traced, `5` Solid + Ray-Traced | `0` |
| `live_view_mode` | `0` Solid, `1` Path-Traced: the Live Render tab's mode | `1` for a file without the key |
| `editor_auto_solve_budget_ms` | milliseconds; a design whose last solve was quicker is re-solved automatically after an edit, `0` turns auto-solve off | `300` |
| `editor_dock_width`, `editor_inspector_height` | Edit tab layout in logical pixels | `640.0`, `340.0` |
| `editor_settings_collapsed`, `editor_inspector_collapsed`, `editor_tier_table_collapsed`, `editor_remap_collapsed` | `true` / `false`: which Edit tab sections are folded | `false` |
| `editor_layout_touched` | `true` once you have changed the Edit tab layout | `false` |
| `recent_native_files` | up to 10 design file paths (`.psdd`), most recent first | empty |
| `suppressed_confirmations` | keys of the confirmation prompts you ticked "Don't ask again" on | empty |
| `render_body_color_override` | three absorption numbers (each limited to 0..=10) for the Stone tab's color of a view that does not show the open design; absent means "Material default" | absent |

**View modes are saved but not restored.** `solid_view_mode` and `live_view_mode` are
written whenever you change the mode, but startup does not apply them
(`gui/startup_settings.rs`): the app always opens on the Live Render tab in Solid mode,
and the Edit tab's Solid viewport starts in Solid as well.

### Remote Endpoint (`RemoteEndpoint`)

The viewer has exactly one remote: `remote: Option<RemoteEndpoint>`, an
`pleochroma-worker serve` coordinator (rendering through joined workers, its own
`--render` lane, or both). `address` is the coordinator's viewer port (7878 by
default), never a joined worker's.

```rust
pub struct RemoteEndpoint {
    pub connection: WorkerSettings,
    pub export_transfer: ExportTransfer, // FullData (default) | FinalPicture
    pub live_transfer: LiveTransfer,     // FullData (default) | FinalPicture
}

pub struct WorkerSettings {
    pub name: String,
    pub address: String,             // "host:port"
    pub cert_dir: String,            // directory holding ca.pem / client.pem / client.key
    pub transfer_mode: TransferMode, // LiveProgressive | FinalOnly
    pub cadence_ms: u32,             // default 500; clamped UP to the worker's advertised floor
    pub preview_scale: PreviewScale, // Full | Half | Quarter | Custom(1..=100)
}
```

`cert_dir` should point at exactly what `pleochroma-worker cert issue-client --dir
<pki-dir> --name <label> --out <bundle-dir>` writes — `ca.pem`, `client.pem`,
`client.key` in one directory (see `pleochroma-worker`'s README). Load-balancing
over several machines is the coordinator's job, not the viewer's. Render
width/height is **not** part of the endpoint — it's session-wide, since the remote
(and the local CPU path) must agree on it for the summed samples to actually
compose correctly.

`export_transfer` is the default of the export dialog's and the tilt video's
"Transfer" choice; `live_transfer` is the Render & Output tab's Live Transfer (shown read-only there and
edited in the Remote dialog, where "Live Transfer" is one
three-way control: progressive, full data when finished, final picture) -- see
[remote-rendering.md](remote-rendering.md) for what "final picture" does.

`contribute_to_final_picture` (default `true`, top-level, not on `RemoteEndpoint`)
is the Render & Output tab's "Final-Picture Exports: This Machine Renders a Share Too"
switch: with a remote configured and `export_transfer`/the tilt video's own
"Transfer" pill at `FinalPicture`, the viewer's own idle CPU/GPU traces a share of
the sample budget alongside the remote and uploads it as one `CONTRIBUTION`, which
the coordinator folds in before tone-mapping. Only takes effect when the export's
"Compute" choice is `Both`; a scene mismatch or an HDR map the local tracer can't
resolve identically silently falls back to a remote-only picture instead of
refusing the export.

`payload_encoding` (default `"auto"`, top-level) is how the viewer compresses what it
uploads to a coordinator -- today the `CONTRIBUTION` of a final-picture export (its own
share of the samples). `"auto"` follows the measured speed of the link (zstd or LZ4 on a
slow one, raw on a fast one or a loopback coordinator), starting each connection from the
speed last measured to the same coordinator address; `"raw"`, `"lz4"`, `"zstd"` (level 1)
and `"zstd:LEVEL"` (1 to 22) pin one encoding on every link. A missing key loads as
`"auto"`, and an unreadable value loads as `"auto"` with a warning rather than failing the
file. It is read from the file at startup (there is no dialog control), so restart after
editing it. The coordinator's own choice for what it sends the viewer is
`pleochroma-worker serve --payload-encoding`.

`import_preview_choice` (default `"ask"`, top-level; `"full"`, `"solid"`, `"ray"` or `"skip"`) is
the remembered answer to the question asked after an import about generating catalogue
previews, stored by the question's "Remember My Choice" box. Set it back to `"ask"` to be
asked again. `"ray"` makes quick local Ray-Traced drafts; these never satisfy the persistent thumbnail renderer choice.

`remote_batch_lanes` (default `4`, top-level, limited to 1..=32 on load and on set) is
the remote worker dialog's "Remote Lanes for Batches" spin box: how many pictures
(preview batch) or designs (tilt batch) a catalogue batch keeps in flight on the remote
at once, one remote dispatcher per lane. Read when a batch starts. Against a
coordinator, keep it at or below the coordinator's `--jobs-per-viewer` (default 8):
requests beyond that wait in its per-viewer queue without progress, and one that waits
there longer than 60 s is given up by the desktop (rendered locally under
"Compute: Both", counted as failed under "Remote Only").

`remote_batch_lanes_user_set` (default `false`, set to `true` by the spin box) records
that the user chose a lane count. Until then a preview batch against a remote GPU worker
uses 2 dispatchers instead of `remote_batch_lanes` (a CPU worker or coordinator keeps it):
the batched requests below keep the GPU busy with few connections. `remote_preview_batch_size`
(default `10`, limited to 1..=32) is the "Pictures per Preview Request" spin box: how many
pictures one protocol v24 `BatchRenderRequest` carries; each dispatcher keeps two such
requests in flight on one persistent connection. A coordinator takes no batches, so
previews against one are sent a picture at a time.

**Migration.** A file written before the single-endpoint model carries a
`[[settings.remote_workers]]` list. It still loads: the FIRST entry becomes `remote` (it was the only one any
feature used), the rest are dropped and logged by address, and the next save writes
only `[settings.remote]`.

## Ray-Traced View

Ray-Traced is an explicit view choice; startup still opens the Live Render tab in Solid. The existing `solid_view_mode` stores 0 Solid, 1 Path-Traced, 2 Solid + Path-Traced, 3 Diagram, 4 Ray-Traced or 5 Solid + Ray-Traced. `live_view_mode` stores 0 Solid, 1 Path-Traced or 2 Ray-Traced. Both saved mode values track the session but are deliberately not restored at startup.

`[settings.ray_trace]` stores `live_taps = 8` (fixed anti-aliasing taps, snapped to 1, 4, 8 or 16) and `fire = true` (three dispersion chains). These controls appear under Ray-Traced View in Advanced Render & Output and reset with that tab. They do not change the Path-Traced sample target or denoise preference. Ray-Traced itself never denoises. Its banded spectral tint, approximate frost and omitted optical effects are described beside the controls.


## Thumbnail and output renderers

`thumbnail_renderer` defaults to `"PathTraced"`; choose `"RayTraced"` through **Thumbnail Renderer** in Advanced Render & Output or the preview offer's Picture Quality block. Ray thumbnails use 8 fixed AA taps with Fire on. Changing this choice marks cached thumbnails outdated once; Regenerate and startup offers compare against the selected renderer. Quick (Ray-Traced) remains a local CPU draft and never counts as a current persistent thumbnail.

`thumbnail_backdrop` (`"Grey"` by default; `"White"` or `"Black"`) is the tray catalogue thumbnails are rendered on, independent of the live view's `backdrop`. It is part of the preview fingerprint (`backdrop=grey`), so changing it marks cached thumbnails outdated once. Set through **Thumbnail Backdrop** in Advanced Render & Output or the preview offer's Picture Quality block.

The still export and tilt-video dialogs each remember their own `engine` alongside their other output settings. Both default to Path-Traced; switching to Ray-Traced exposes Anti-Aliasing (1, 4, 8 or 16 taps, initially 16) and Fire. This choice is independent of the live view and thumbnail renderer. Output jobs freeze the engine and its AA/Fire settings: Path-Traced jobs with no shadow and no reflection remain format 1 and Ray-Traced jobs use format 2; the shadow alone makes a job use format 3, and any job with "Tray Reflection" above 0 uses format 4, with or without the shadow (a stone with color zones makes a Path-Traced job format 5, and a turned HDR map format 6, whatever else it holds). Local, Remote and Both compute choices work for both renderers with protocol-42 workers. Ray-Traced output never denoises or adds optical overlays; metric-curve annotations on tilt videos still use the independent analytical tracer.
