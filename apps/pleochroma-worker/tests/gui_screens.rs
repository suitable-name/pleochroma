//! Screenshot harness for the coordinator window (custom `main`, `harness = false`).
//!
//! It writes one PNG for every page and dialog, in the normal colors and in high contrast, at
//! 1180 x 760 and at the smallest window, 640 x 480.
//!
//! It opens the real `AppWindow` on Slint's testing backend with the software renderer and fills
//! the exported globals (`ShellState`, the page states, `PairState`, `WizardState`, ...) directly.
//! No coordinator, no backend and no settings file are involved: nothing is wired, so no button
//! does anything and the pictures show exactly the state this file sets.
//!
//! Run (the owner runs it; it needs no library and no network):
//!
//! ```text
//! cargo test -p pleochroma-worker --test gui_screens
//! cargo test -p pleochroma-worker --test gui_screens -- overview pair   (only these screens)
//! ```
//!
//! Arguments that do not start with `-` select screens by name substring. The pictures are
//! written to `<target>/tmp/gui-screens/<screen>_<normal|high-contrast>_<width>x<height>.png`
//! and every file is announced on standard output.
//!
//! Needs, in `apps/pleochroma-worker/Cargo.toml`: `harness = false` on this `[[test]]` and
//! `uiprobe = { workspace = true, features = ["screenshots"] }` under `[dev-dependencies]`.
//!
//! The dialogs that live inside one page (Rename and Remove access on Devices, Delete on Jobs)
//! keep their open state in the page and cannot be opened from outside; they are not shot here.

use pleochroma_worker::gui::app::{
    AccessRowData, AddState, AppWindow, DeviceRowData, DevicesState, JobRowData, JobsState,
    LibraryState, LogState, OvActivityRow, OvLiveRow, OverviewState, PairState, RcAllowedRow,
    RcConnectedRow, RenderState, SettingsState, ShellState, SwitcherRow, SwitcherState, Theme,
    ToolsState, WizardState, WorkerModeState,
};
use slint::{
    ComponentHandle, Image, LogicalPosition, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
    platform::{PointerEventButton, WindowEvent},
};
use std::path::Path;
use uiprobe::{prelude::*, screenshot};

/// The two window sizes of every screen, in logical pixels: the usual one and the smallest the
/// window allows.
const SIZES: [(u32, u32); 2] = [(1180, 760), (640, 480)];

/// Physical pixels per logical pixel.
const SCALE: f32 = 1.0;

/// One screen: its file name stem, the state it needs and what it does after the window has its
/// size (a click that opens a popup).
struct Screen {
    /// The file name stem.
    name: &'static str,
    /// Fills the globals.
    setup: fn(&AppWindow),
    /// Runs once the window has its size, before the picture is taken.
    act: fn(&AppWindow, (u32, u32)),
}

/// Every screen, in the order it is taken.
const SCREENS: [Screen; 41] = [
    screen("overview", overview),
    screen("devices", devices),
    screen("render_computers", render_computers),
    screen("render_add_dialog", render_add_dialog),
    screen("jobs", jobs),
    screen("library", library),
    screen("library_rendering_only", library_rendering_only),
    screen("log", log),
    screen("settings_general", settings_general),
    screen("settings_network", settings_network),
    screen("settings_security_remote_admin", settings_security),
    screen("settings_performance", settings_performance),
    screen("settings_storage", settings_storage),
    screen("settings_advanced", settings_advanced),
    screen("settings_backup_dialog", settings_backup_dialog),
    screen("settings_confirm_dialog", settings_confirm_dialog),
    screen("settings_code_dialog", settings_code_dialog),
    screen("help_another_coordinator", help_another),
    screen("tools", tools),
    screen("wizard_start", wizard_start),
    screen("wizard_name", wizard_name),
    screen("wizard_library", wizard_library),
    screen("wizard_network", wizard_network),
    screen("wizard_rendering", wizard_rendering),
    screen("wizard_done", wizard_done),
    screen("wizard_monitor_done", wizard_monitor_done),
    screen("pair_phone", pair_phone),
    screen("pair_computer", pair_computer),
    screen("pair_paired", pair_paired),
    screen("pair_expired", pair_expired),
    screen("add_coordinator_link", add_coordinator_link),
    screen("add_coordinator_code", add_coordinator_code),
    Screen {
        name: "switcher",
        setup: switcher,
        act: open_switcher,
    },
    screen("forget_dialog", forget_dialog),
    Screen {
        name: "more_menu",
        setup: overview,
        act: open_more_menu,
    },
    screen("monitor_overview", monitor_overview),
    screen("monitor_devices", monitor_devices),
    screen("monitor_render_computers", monitor_render),
    screen("monitor_jobs", monitor_jobs),
    screen("monitor_library", monitor_library),
    screen("monitor_settings", monitor_settings),
];

/// A screen with nothing to do after the window has its size.
const fn screen(name: &'static str, setup: fn(&AppWindow)) -> Screen {
    Screen {
        name,
        setup,
        act: nothing,
    }
}

/// The empty `act` of most screens.
const fn nothing(_window: &AppWindow, _size: (u32, u32)) {}

fn main() -> ExitCode {
    let mut harness = Harness::new().renderer(Renderer::Software);
    let out = uiprobe::target_dir!().join("gui-screens");
    // Names given on the command line (without a leading dash) pick screens by substring.
    let wanted: Vec<String> = std::env::args()
        .skip(1)
        .filter(|argument| !argument.starts_with('-'))
        .collect();
    for screen in SCREENS {
        if !wanted.is_empty()
            && !wanted
                .iter()
                .any(|word| screen.name.contains(word.as_str()))
        {
            continue;
        }
        let out = out.clone();
        harness = harness.scenario(screen.name, move |_ctx| shoot(&screen, &out));
    }
    harness.run()
}

/// Takes the four pictures of one screen (two contrasts, two sizes), each in a fresh window.
fn shoot(screen: &Screen, out: &Path) -> Result<(), Finding> {
    for high_contrast in [false, true] {
        for size in SIZES {
            let window = AppWindow::new().map_err(|e| {
                Finding::simple("launch", format!("{}: cannot start: {e}", screen.name))
            })?;
            window.show().map_err(|e| {
                Finding::simple("launch", format!("{}: cannot show: {e}", screen.name))
            })?;
            window.global::<Theme>().set_high_contrast(high_contrast);
            (screen.setup)(&window);
            screenshot::set_window_size(window.window(), size, SCALE);
            pump(300);
            (screen.act)(&window, size);
            pump(300);
            let picture = screenshot::capture(window.window())
                .map_err(|e| Finding::simple("shot", format!("{}: {e}", screen.name)))?;
            let contrast = if high_contrast {
                "high-contrast"
            } else {
                "normal"
            };
            let file = out.join(format!(
                "{}_{contrast}_{}x{}.png",
                screen.name, size.0, size.1
            ));
            picture
                .save_png(&file)
                .map_err(|e| Finding::simple("shot", format!("{}: {e}", screen.name)))?;
            println!(
                "gui-screens: wrote {} ({} x {})",
                file.display(),
                picture.width(),
                picture.height()
            );
            let _ = window.hide();
            pump(50);
        }
    }
    Ok(())
}

/// A string for a Slint property.
fn s(text: &str) -> SharedString {
    SharedString::from(text)
}

/// A model for a Slint array property.
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// A stand-in pairing code picture: a 33 x 33 pattern with the three finder squares, black on
/// white (the window draws the white card itself).
fn qr_image() -> Image {
    const N: u32 = 33;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(N, N);
    let pixels = buffer.make_mut_slice();
    for y in 0..N {
        for x in 0..N {
            let finder = |cx: u32, cy: u32| {
                let dx = x.abs_diff(cx);
                let dy = y.abs_diff(cy);
                let ring = dx.max(dy);
                (x < cx + 4 && y < cy + 4 && x + 4 > cx && y + 4 > cy) && (ring == 3 || ring <= 1)
            };
            let on = finder(3, 3)
                || finder(N - 4, 3)
                || finder(3, N - 4)
                || (x * 7 + y * 13 + x * y) % 5 < 2;
            let value = if on { 0 } else { 255 };
            if let Some(pixel) = pixels.get_mut((y * N + x) as usize) {
                *pixel = Rgba8Pixel {
                    r: value,
                    g: value,
                    b: value,
                    a: 255,
                };
            }
        }
    }
    Image::from_rgba8(buffer)
}

/// Presses and releases the left button at `(x, y)`.
fn click(window: &AppWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    pump(150);
}

/// The running shell of a local coordinator, showing page `page`.
fn shell(window: &AppWindow, page: i32) {
    let shell = window.global::<ShellState>();
    shell.set_page(page);
    shell.set_status_text(s("Running"));
    shell.set_status_level(0);
    shell.set_running(true);
    shell.set_busy(false);
    shell.set_can_pair(true);
    shell.set_coordinator_name(s("Studio coordinator"));
    shell.set_coordinator_place(s("This computer"));
    shell.set_devices_badge(s("2"));
    shell.set_jobs_badge(s("1"));
}

/// The shell of a remote coordinator opened with monitor access, showing page `page`.
fn monitor_shell(window: &AppWindow, page: i32) {
    shell(window, page);
    let shell = window.global::<ShellState>();
    shell.set_remote(true);
    shell.set_status_text(s("Running · remote"));
    shell.set_coordinator_name(s("Garage coordinator"));
    shell.set_coordinator_place(s("Remote · Monitor only"));
    shell.set_can_pair(false);
    shell.set_denied_reason(s("You have monitor access. Ask the owner of this coordinator for an admin code to change things."));
    shell.set_banner_text(s("Managing Garage coordinator remotely · Monitor"));
    shell.set_banner_tone(0);
}

// ---- The pages -------------------------------------------------------------------------------

fn overview(window: &AppWindow) {
    shell(window, 0);
    let o = window.global::<OverviewState>();
    o.set_loaded(true);
    o.set_hero_title(s("Running · phones on your network can connect"));
    o.set_hero_level(0);
    o.set_hero_sub(s("Ports 7878 · 7879 · 7880 · 7881"));
    o.set_address(s("192.168.178.40"));
    o.set_uptime(s("3 h 12 min"));
    o.set_devices_value(s("2 of 3"));
    o.set_devices_sub(s("Anna's phone, Studio laptop"));
    o.set_computers_value(s("1 + this computer"));
    o.set_computers_sub(s("garage-rig online"));
    o.set_jobs_value(s("1 running"));
    o.set_jobs_sub(s("2 queued"));
    o.set_live_note(s("1 active"));
    o.set_live(model(vec![OvLiveRow {
        owner: s("Anna's phone"),
        label: s("Final render"),
        size: s("3840 × 2160"),
        progress: 0.62,
        percent: s("62 %"),
        route: s("This computer + garage-rig"),
        samples: s("318 of 512 samples"),
        rate: s("4.2 M samples/s"),
        elapsed: s("2 min"),
        determinate: true,
        preparing: false,
    }]));
    o.set_activity(model(vec![
        OvActivityRow {
            when: s("just now"),
            text: s("Anna's phone started a render"),
            tone: 0,
        },
        OvActivityRow {
            when: s("5 min ago"),
            text: s("Studio laptop paired"),
            tone: 1,
        },
        OvActivityRow {
            when: s("1 h ago"),
            text: s("garage-rig lost its connection and came back"),
            tone: 2,
        },
        OvActivityRow {
            when: s("2 h ago"),
            text: s("A render failed: out of memory"),
            tone: 3,
        },
    ]));
    o.set_warnings(model(vec![s(
        "Windows Firewall may block phones. Allow Pleochroma on private networks.",
    )]));
    o.set_can_pair(true);
}

/// Three devices for the Devices page.
fn device_rows() -> Vec<DeviceRowData> {
    vec![
        DeviceRowData {
            name: s("Anna's phone"),
            kind: s("Phone · Android"),
            icon: 0,
            online: true,
            since: s("Connected since 14:02"),
            now: s("Rendering Final render"),
            addr: s("192.168.178.52"),
            fingerprint: s("a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"),
            fingerprint_short: s("a1b2c3d4"),
        },
        DeviceRowData {
            name: s("Studio laptop"),
            kind: s("Computer · Windows"),
            icon: 1,
            online: true,
            since: s("Connected since 09:30"),
            now: s("Idle"),
            addr: s("192.168.178.61"),
            fingerprint: s("b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1"),
            fingerprint_short: s("b2c3d4e5"),
        },
        DeviceRowData {
            name: s("Garage coordinator"),
            kind: s("Remote administration · Admin"),
            icon: 2,
            online: false,
            since: s("Last seen yesterday 21:40"),
            now: s("—"),
            addr: s(""),
            fingerprint: s("c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2"),
            fingerprint_short: s("c3d4e5f6"),
        },
    ]
}

fn devices(window: &AppWindow) {
    shell(window, 1);
    let d = window.global::<DevicesState>();
    d.set_devices(model(device_rows()));
    d.set_heading(s("3 devices can connect"));
    d.set_online_text(s("2 online now"));
}

/// Two joined render computers and one allowed one.
fn render_rows(window: &AppWindow) {
    let r = window.global::<RenderState>();
    r.set_loaded(true);
    r.set_own_enabled(true);
    r.set_own_mode(0);
    r.set_own_threads(0);
    r.set_own_hardware(s("Radeon 780M · 16 threads"));
    r.set_own_level(0);
    r.set_own_state(s("Idle"));
    r.set_connected(model(vec![RcConnectedRow {
        id: 1,
        name: s("garage-rig"),
        kind: s("Worker · Windows"),
        hardware: s("RTX 4070"),
        hardware_sub: s("24 threads · 2 slots"),
        busy: true,
        state: s("Busy"),
        state_sub: s("1 of 2 slots busy"),
        since: s("Connected for 12 min"),
        rate: s("12.5 samples per second"),
        addr: s("192.168.178.70"),
    }]));
    r.set_allowed(model(vec![RcAllowedRow {
        name: s("attic-box"),
        kind: s("Worker · Linux"),
        added: s("Added 3 days ago"),
        online: false,
        state: s("Offline"),
        state_sub: s("Will join when it starts"),
        seen: s("Last seen 2 h ago"),
        addr: s(""),
    }]));
    r.set_capacity(s("2 GPUs · 40 threads · up to 7680 × 4320"));
}

fn render_computers(window: &AppWindow) {
    shell(window, 2);
    render_rows(window);
}

fn render_add_dialog(window: &AppWindow) {
    render_computers(window);
    let r = window.global::<RenderState>();
    r.set_add_open(true);
    r.set_add_phase(1);
    r.set_add_qr(qr_image());
    r.set_add_link_shown(s(
        "pleochroma://worker?host=192.168.178.40&port=7881&code=••••••••",
    ));
    r.set_add_fraction(0.7);
    r.set_add_remaining(s("2:06"));
}

fn jobs_rows() -> Vec<JobRowData> {
    vec![
        JobRowData {
            id: s("0001"),
            owner: s("Anna's phone"),
            label: s("Final render"),
            detail: s("Emerald oval"),
            detail_bad: false,
            size: s("4096 × 2304"),
            samples: s("645 / 1024"),
            state: 1,
            state_text: s("Running"),
            progress: 0.63,
            progress_text: s("63%"),
            created: s("12 min ago"),
            finished: s(""),
            route: s("this computer + garage-rig"),
            can_cancel: true,
            can_delete: false,
        },
        JobRowData {
            id: s("0002"),
            owner: s("Studio laptop"),
            label: s("Preview"),
            detail: s("Round brilliant"),
            detail_bad: false,
            size: s("1920 × 1080"),
            samples: s("0 / 256"),
            state: 0,
            state_text: s("Queued"),
            progress: 0.0,
            progress_text: s("waiting"),
            created: s("3 min ago"),
            finished: s(""),
            route: s(""),
            can_cancel: true,
            can_delete: false,
        },
        JobRowData {
            id: s("0003"),
            owner: s("Anna's phone"),
            label: s("Test"),
            detail: s("Out of memory on garage-rig"),
            detail_bad: true,
            size: s("8192 × 4608"),
            samples: s("12 / 512"),
            state: 3,
            state_text: s("Failed"),
            progress: 0.02,
            progress_text: s("2%"),
            created: s("1 h ago"),
            finished: s("Finished 1 h ago"),
            route: s(""),
            can_cancel: false,
            can_delete: true,
        },
        JobRowData {
            id: s("0004"),
            owner: s("Studio laptop"),
            label: s("Hero shot"),
            detail: s("Sapphire cushion"),
            detail_bad: false,
            size: s("3840 × 2160"),
            samples: s("1024 / 1024"),
            state: 2,
            state_text: s("Done"),
            progress: 1.0,
            progress_text: s("100%"),
            created: s("yesterday"),
            finished: s("Finished yesterday"),
            route: s(""),
            can_cancel: false,
            can_delete: true,
        },
    ]
}

fn jobs(window: &AppWindow) {
    shell(window, 3);
    let j = window.global::<JobsState>();
    let rows = jobs_rows();
    j.set_running(model(rows.iter().take(1).cloned().collect()));
    j.set_jobs(model(rows));
    j.set_count_all(4);
    j.set_count_running(1);
    j.set_count_queued(1);
    j.set_count_done(1);
    j.set_count_failed(1);
    j.set_storage_text(s(
        "Results are kept 7 days · 4 of 256 jobs · 412 MB of 8 GB",
    ));
    j.set_has_folder(true);
}

fn library(window: &AppWindow) {
    shell(window, 4);
    let l = window.global::<LibraryState>();
    l.set_has_library(true);
    l.set_path(s("D:\\Gems\\facet_diagrams.sqlite"));
    l.set_designs_text(s("4,812"));
    l.set_read_only(true);
}

fn library_rendering_only(window: &AppWindow) {
    shell(window, 4);
    let l = window.global::<LibraryState>();
    l.set_has_library(false);
    l.set_restart_pending(true);
    l.set_pending_text(s("no library"));
}

fn log(window: &AppWindow) {
    shell(window, 6);
    let l = window.global::<LogState>();
    l.set_loaded(true);
    l.set_times(model(vec![
        s("14:02:11"),
        s("14:02:12"),
        s("14:05:40"),
        s("14:07:03"),
    ]));
    l.set_levels(model(vec![2, 2, 1, 0]));
    l.set_messages(model(vec![
        s("Coordinator started on 192.168.178.40:7878"),
        s("Anna's phone connected"),
        s("garage-rig answered slowly"),
        s("A render failed: out of memory"),
    ]));
    l.set_count_text(s("showing 4 of 1,284 lines · Info and above"));
    l.set_kept_text(s("Times in UTC · Kept for 14 days · 3.4 MB"));
}

/// Opens the Settings page on `section`.
fn settings_at(window: &AppWindow, section: i32) {
    shell(window, 5);
    let t = window.global::<SettingsState>();
    t.set_loaded(true);
    t.set_section(section);
    t.set_name(s("Studio coordinator"));
    t.set_start_at_login(true);
    t.set_keep_awake(true);
    t.set_update_check(true);
    t.set_listen(1);
    t.set_port(s("7878"));
    t.set_port_viewer_enroll(s("7879"));
    t.set_port_worker(s("7880"));
    t.set_port_worker_enroll(s("7881"));
    t.set_max_connections(64);
    t.set_max_per_address(8);
    t.set_discovery(true);
    t.set_net_profile(s("Private network"));
    t.set_net_profile_level(0);
    t.set_net_firewall(s("Pleochroma is allowed"));
    t.set_net_firewall_level(0);
    t.set_net_contact(s("Anna's phone, 5 min ago"));
    t.set_net_contact_level(0);
    t.set_allow_pairing(true);
    t.set_cert_names(s("studio.example.com"));
    t.set_job_memory(2048);
    t.set_whole_seconds(10);
    t.set_whole_megapixels(64);
    t.set_jobs_per_device(4);
    t.set_interactive_workers(2);
    t.set_compression(0);
    t.set_accept_workers(true);
    t.set_cpu_threads(0);
    t.set_retention_days(7);
    t.set_cache_mib(1024);
    t.set_data_folder(s(
        "C:\\Users\\Studio\\AppData\\Roaming\\Pleochroma Coordinator",
    ));
    t.set_jobs_folder(s(
        "C:\\Users\\Studio\\AppData\\Roaming\\Pleochroma Coordinator\\jobs",
    ));
    t.set_cache_folder(s(
        "C:\\Users\\Studio\\AppData\\Roaming\\Pleochroma Coordinator\\cache",
    ));
    t.set_log_folder(s(
        "C:\\Users\\Studio\\AppData\\Roaming\\Pleochroma Coordinator\\logs",
    ));
}

fn settings_general(window: &AppWindow) {
    settings_at(window, 0);
}

fn settings_network(window: &AppWindow) {
    settings_at(window, 1);
}

fn settings_security(window: &AppWindow) {
    settings_at(window, 2);
    let t = window.global::<SettingsState>();
    t.set_remote_admin(true);
    t.set_remote_admin_active(true);
    t.set_access_rows(model(vec![
        AccessRowData {
            name: s("Laptop 2"),
            access: s("Full administration"),
            seen: s("Today 09:12"),
            online: true,
            fingerprint: s("d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3"),
        },
        AccessRowData {
            name: s("Office monitor"),
            access: s("Monitor only"),
            seen: s("Never"),
            online: false,
            fingerprint: s("e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4"),
        },
    ]));
}

fn settings_performance(window: &AppWindow) {
    settings_at(window, 3);
}

fn settings_storage(window: &AppWindow) {
    settings_at(window, 4);
}

fn settings_advanced(window: &AppWindow) {
    settings_at(window, 5);
}

fn settings_backup_dialog(window: &AppWindow) {
    settings_at(window, 5);
    window.global::<SettingsState>().set_dialog(1);
}

fn settings_confirm_dialog(window: &AppWindow) {
    settings_at(window, 5);
    window.global::<SettingsState>().set_dialog(3);
}

fn settings_code_dialog(window: &AppWindow) {
    settings_security(window);
    let t = window.global::<SettingsState>();
    t.set_code_for(s("Laptop 2"));
    t.set_code_access(s("Full administration"));
    t.set_code_link_shown(s(
        "pleochroma://pair?host=192.168.178.40&port=7879&code=••••••••",
    ));
    t.set_code_remaining(s("2:41"));
    t.set_dialog(4);
}

fn help_another(window: &AppWindow) {
    shell(window, 7);
    let w = window.global::<WorkerModeState>();
    w.set_loaded(true);
    w.set_link_state(1);
    w.set_link_message(s("link valid"));
    w.set_link_coordinator(s("Garage coordinator · 192.168.178.40:7880"));
    w.set_saved_text(s("Garage coordinator · 192.168.178.40:7880"));
    w.set_hardware(s("Radeon 780M · 16 threads"));
    w.set_cert_folder(s(
        "C:\\Users\\Studio\\AppData\\Roaming\\Pleochroma Coordinator\\worker",
    ));
}

fn tools(window: &AppWindow) {
    shell(window, 8);
    let t = window.global::<ToolsState>();
    t.set_loaded(true);
    t.set_scene_path(s("C:\\Scenes\\ring.json"));
    t.set_out_path(s("C:\\Scenes\\ring.png"));
    t.set_command(s(
        "pleochroma-worker --headless serve --listen 0.0.0.0 --port 7878 --library D:\\Gems\\facet_diagrams.sqlite",
    ));
}

// ---- The dialogs -----------------------------------------------------------------------------

/// Opens the first-run wizard on `step` with a monitoring choice of `mode`.
fn wizard_at(window: &AppWindow, step: i32, mode: i32) {
    shell(window, 0);
    let w = window.global::<WizardState>();
    w.set_open(true);
    w.set_mode(mode);
    w.set_step(step);
    w.set_name(s("Studio coordinator"));
    w.set_library_path(s("D:\\Gems\\facet_diagrams.sqlite"));
    w.set_network_address(s("192.168.178.40"));
    w.set_gpu_note(s("Radeon 780M found"));
}

fn wizard_start(window: &AppWindow) {
    wizard_at(window, 0, 0);
}

fn wizard_name(window: &AppWindow) {
    wizard_at(window, 1, 0);
}

fn wizard_library(window: &AppWindow) {
    wizard_at(window, 2, 0);
}

fn wizard_network(window: &AppWindow) {
    wizard_at(window, 3, 0);
}

fn wizard_rendering(window: &AppWindow) {
    wizard_at(window, 4, 0);
}

fn wizard_done(window: &AppWindow) {
    wizard_at(window, 5, 0);
}

fn wizard_monitor_done(window: &AppWindow) {
    wizard_at(window, 5, 1);
}

/// Opens the pairing dialog on tab `mode` in `phase`.
fn pair_at(window: &AppWindow, mode: i32, phase: i32) {
    shell(window, 0);
    let p = window.global::<PairState>();
    p.set_open(true);
    p.set_mode(mode);
    p.set_phase(phase);
    p.set_qr(qr_image());
    p.set_link_shown(s(
        "pleochroma://pair?host=192.168.178.40&port=7879&code=••••••••",
    ));
    p.set_addresses(model(vec![s("192.168.178.40:7879"), s("10.8.0.5:7879")]));
    p.set_fraction(0.62);
    p.set_remaining_text(s("1:52"));
    p.set_coordinator_name(s("Studio coordinator"));
    p.set_device_ip(s("192.168.178.52"));
    p.set_device_name(s("Pixel 8"));
    p.set_device_certificate(s("a1b2c3d4…"));
    p.set_device_access(s("Render and browse"));
}

fn pair_phone(window: &AppWindow) {
    pair_at(window, 0, 1);
}

fn pair_computer(window: &AppWindow) {
    pair_at(window, 1, 1);
}

fn pair_paired(window: &AppWindow) {
    pair_at(window, 0, 2);
}

fn pair_expired(window: &AppWindow) {
    pair_at(window, 0, 3);
}

fn add_coordinator_link(window: &AppWindow) {
    shell(window, 0);
    let a = window.global::<AddState>();
    a.set_open(true);
    a.set_mode(0);
    a.set_link(s(
        "pleochroma://pair?host=192.168.178.41&port=7879&code=••••••••",
    ));
    a.set_link_ok(true);
    a.set_link_name(s("Garage coordinator"));
    a.set_link_place(s("192.168.178.41:7879"));
    a.set_link_access(s("Full administration"));
    a.set_link_expires(s("in 2:41"));
    a.set_status_text(s("The link is valid."));
    a.set_status_level(0);
    a.set_can_connect(true);
}

fn add_coordinator_code(window: &AppWindow) {
    shell(window, 0);
    let a = window.global::<AddState>();
    a.set_open(true);
    a.set_mode(1);
    a.set_address(s("192.168.178.41:7878"));
    a.set_status_text(s("Type the address and the code."));
    a.set_status_level(1);
}

/// Three rows for the coordinator switcher.
fn switcher_rows() -> Vec<SwitcherRow> {
    vec![
        SwitcherRow {
            id: s(""),
            name: s("Studio coordinator"),
            place: s("This computer"),
            tag: s("Online"),
            level: 0,
            selected: true,
            remote: false,
        },
        SwitcherRow {
            id: s("garage"),
            name: s("Garage coordinator"),
            place: s("Remote · Full administration"),
            tag: s("Online"),
            level: 0,
            selected: false,
            remote: true,
        },
        SwitcherRow {
            id: s("attic"),
            name: s("Attic coordinator"),
            place: s("Remote · Monitor only"),
            tag: s("Offline"),
            level: 3,
            selected: false,
            remote: true,
        },
    ]
}

fn switcher(window: &AppWindow) {
    overview(window);
    window
        .global::<SwitcherState>()
        .set_rows(model(switcher_rows()));
}

/// Clicks the coordinator name at the top of the sidebar.
fn open_switcher(window: &AppWindow, size: (u32, u32)) {
    let x = if size.0 < 820 { 28.0 } else { 120.0 };
    click(window, x, 31.0);
}

fn forget_dialog(window: &AppWindow) {
    switcher(window);
    let w = window.global::<SwitcherState>();
    w.set_forget_id(s("garage"));
    w.set_forget_name(s("Garage coordinator"));
    w.set_forget_open(true);
}

/// Clicks the "More" button of the top bar.
fn open_more_menu(window: &AppWindow, size: (u32, u32)) {
    let width = f32::from(u16::try_from(size.0).unwrap_or(u16::MAX));
    click(window, width - 33.0, 30.0);
}

// ---- Monitor-only access to a remote coordinator ---------------------------------------------

fn monitor_overview(window: &AppWindow) {
    overview(window);
    monitor_shell(window, 0);
    window.global::<OverviewState>().set_can_pair(false);
}

fn monitor_devices(window: &AppWindow) {
    devices(window);
    monitor_shell(window, 1);
    let d = window.global::<DevicesState>();
    d.set_can_rename(false);
    d.set_can_remove(false);
    d.set_can_pair(false);
    d.set_denied_reason(s(
        "You have monitor access. Ask the owner for an admin code to change devices.",
    ));
}

fn monitor_render(window: &AppWindow) {
    render_computers(window);
    monitor_shell(window, 2);
    let r = window.global::<RenderState>();
    r.set_can_change(false);
    r.set_can_disconnect(false);
    r.set_can_add(false);
    r.set_denied_reason(s(
        "You have monitor access. Ask the owner for an admin code to change render settings.",
    ));
}

fn monitor_jobs(window: &AppWindow) {
    jobs(window);
    monitor_shell(window, 3);
    let j = window.global::<JobsState>();
    j.set_can_cancel(false);
    j.set_can_delete(false);
    j.set_denied_reason(s(
        "You have monitor access. Ask the owner for an admin code to cancel or delete jobs.",
    ));
}

fn monitor_library(window: &AppWindow) {
    library(window);
    monitor_shell(window, 4);
    let l = window.global::<LibraryState>();
    l.set_can_change(false);
    l.set_can_restart(false);
    l.set_denied_reason(s(
        "The library can only be changed on the computer that serves it.",
    ));
}

fn monitor_settings(window: &AppWindow) {
    monitor_shell(window, 5);
    let t = window.global::<SettingsState>();
    t.set_loaded(true);
    t.set_remote_name(s("Garage coordinator"));
    t.set_remote_display_name(s("Garage coordinator"));
    t.set_remote_allow_pairing(true);
    t.set_remote_discovery(true);
    t.set_remote_can_change(false);
    t.set_remote_can_restart(false);
    t.set_remote_note(s(
        "You have monitor access. Settings can be read but not changed.",
    ));
}
