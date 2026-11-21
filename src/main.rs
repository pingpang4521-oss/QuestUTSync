mod detector;
mod openvr;
mod overlay;
mod pipe;
mod scanner;
mod sleeve_bridge;
mod solver;
mod ui;

use detector::{CalibrationEngine, LogLevel};
use eframe::egui;
use overlay::DashboardOverlay;
use scanner::{RoomScanner, ScanPhase, ServerState};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

struct CliOptions {
    show_help: bool,
    self_test: bool,
    headless: bool,
    run_once: bool,
    scan_room: bool,
    pair_tracker: bool,
    no_autolaunch: bool,
    dry_run: bool,
    feet_back_cm: f32,
}

fn parse_args(args: &[String]) -> Result<CliOptions, String> {
    let mut opts = CliOptions {
        show_help: false,
        self_test: false,
        headless: false,
        run_once: false,
        scan_room: false,
        pair_tracker: false,
        no_autolaunch: false,
        dry_run: false,
        feet_back_cm: 0.0,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].to_ascii_lowercase().as_str() {
            "--help" | "-h" | "/?" | "-?" => {
                opts.show_help = true;
                return Ok(opts);
            }
            "--selftest" => opts.self_test = true,
            "--headless" => opts.headless = true,
            "--once" => {
                opts.run_once = true;
                opts.headless = true;
            }
            "--scan" => opts.scan_room = true,
            "--pair" => opts.pair_tracker = true,
            "--no-autolaunch" => opts.no_autolaunch = true,
            "--dry-run" => opts.dry_run = true,
            "--feet-back-cm" => {
                let Some(val_str) = args.get(i + 1) else {
                    return Err("Missing value for --feet-back-cm".to_string());
                };
                let cm: f32 = val_str
                    .parse()
                    .map_err(|_| format!("Invalid number for --feet-back-cm: {val_str}"))?;
                if !cm.is_finite() || cm.abs() > 30.0 {
                    return Err("--feet-back-cm must be between -30 and 30".to_string());
                }
                opts.feet_back_cm = cm;
                i += 1;
            }
            other => {
                // Never ignore unknown args: a typo like "--dryrun" would otherwise write to VIVE Hub for real.
                return Err(format!("Unknown or invalid argument: {other}"));
            }
        }
        i += 1;
    }

    Ok(opts)
}

fn print_help() {
    println!("============================================================");
    println!(" QuestUTSync - Automatic Vive Ultimate Tracker Calibration");
    println!("============================================================");
    println!("Usage: tracker_setup_lite.exe [options]\n");
    println!("Options:");
    println!("  --scan              Run a hands-free room scan (no HTC app/headset) and exit");
    println!("  --pair              Put the next free dongle slot into pairing mode and exit");
    println!("  --no-autolaunch     Do not auto-launch ViveTrackerServer.exe silently");
    println!("  --headless          Run in console mode without GUI/overlay");
    println!("  --once              Calibrate once (headless) and exit immediately");
    println!("  --dry-run           Compute alignment without sending to VIVE Hub pipe");
    println!("  --feet-back-cm <n>  Place feet n cm behind the headset (default 0, range -30..30)");
    println!("  --selftest          Run built-in verification math & ABI test suite");
    println!("  --help, -h          Show this help screen\n");
    println!("In-VR Calibration Procedure:");
    println!("  1. Stand upright and look straight ahead for 1s (Single beep = Armed).");
    println!("  2. Lift your LEFT foot up 10+ cm (Double beep = Synced!).");
}

fn try_relaunch_with_allowed_pipe_name(raw_args: &[String]) -> Option<ExitCode> {
    if std::env::var_os("QUESTUTSYNC_ALIAS_ACTIVE").as_deref() == Some("1".as_ref()) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_string_lossy();
    if name.eq_ignore_ascii_case("tracker_setup_lite.exe")
        || name.eq_ignore_ascii_case("tracker_setup.exe")
        || name.eq_ignore_ascii_case("vrserver.exe")
    {
        return None;
    }

    let staged_exe = match stage_alias_executable(&exe) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("[Launcher Warning] Could not stage a copy named tracker_setup_lite.exe: {err}");
            eprintln!("[Launcher Warning] Running unchanged. VIVE Hub only accepts that name, so the sync may be ignored.");
            return None;
        }
    };

    let status = match Command::new(&staged_exe)
        .args(raw_args)
        .env("QUESTUTSYNC_ALIAS_ACTIVE", "1")
        .status()
    {
        Ok(s) => s,
        Err(err) => {
            eprintln!("[Launcher Warning] Could not start '{}': {err}", staged_exe.display());
            return None;
        }
    };
    let code = status.code().unwrap_or(1).clamp(0, 255) as u8;
    Some(ExitCode::from(code))
}

/// Stages the executable into `%TEMP%\QuestUTSync\<size>-<mtime>\tracker_setup_lite.exe`
/// so a concurrent launch of the same build reuses the existing copy instead of failing
/// to overwrite a running binary.
fn stage_alias_executable(exe: &std::path::Path) -> Result<PathBuf, std::io::Error> {
    let src_meta = std::fs::metadata(exe)?;
    let mtime_ns = src_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let stage_root = std::env::temp_dir().join("QuestUTSync");
    let stage_dir = stage_root.join(format!("{:x}-{:x}", src_meta.len(), mtime_ns));
    std::fs::create_dir_all(&stage_dir)?;

    let staged_exe = stage_dir.join("tracker_setup_lite.exe");
    let is_same = |p: &std::path::Path| -> bool {
        std::fs::metadata(p).ok().is_some_and(|m| m.len() == src_meta.len())
    };
    if !is_same(&staged_exe) {
        if let Err(err) = std::fs::copy(exe, &staged_exe) {
            if !is_same(&staged_exe) {
                return Err(err);
            }
        }
    }

    // Stage openvr_api.dll next to the aliased exe if present next to the original exe.
    if let Some(orig_dir) = exe.parent() {
        let src_dll = orig_dir.join("openvr_api.dll");
        let dst_dll = stage_dir.join("openvr_api.dll");
        if src_dll.exists() && !dst_dll.exists() {
            let _ = std::fs::copy(&src_dll, &dst_dll);
        }
    }

    // Best-effort cleanup of stale staging folders and legacy flat copy.
    let _ = std::fs::remove_file(stage_root.join("tracker_setup_lite.exe"));
    if let Ok(entries) = std::fs::read_dir(&stage_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path != stage_dir {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }

    Ok(staged_exe)
}

fn run_cli_pair(opts: &CliOptions) -> ExitCode {
    println!("============================================================");
    println!(" QuestUTSync - Dongle Tracker Pairing (No HTC App Required)");
    println!("============================================================");
    let scanner = RoomScanner::new(!opts.no_autolaunch, opts.dry_run, true);
    let start_wait = Instant::now();
    loop {
        for (_, msg) in scanner.drain_logs() {
            println!("{msg}");
        }
        let snap = scanner.snapshot();
        match snap.server_state {
            ServerState::Running => break,
            ServerState::NotInstalled => {
                eprintln!("[Error] ViveTrackerServer.exe not found.");
                return ExitCode::from(1);
            }
            ServerState::Offline if start_wait.elapsed() >= Duration::from_secs(14) => {
                eprintln!("[Error] ViveTrackerServer.exe did not start within 14s.");
                return ExitCode::from(1);
            }
            _ => std::thread::sleep(Duration::from_millis(150)),
        }
    }

    scanner.pair_tracker(None);
    let wait = Instant::now();
    while wait.elapsed() < Duration::from_secs(6) {
        let logs = scanner.drain_logs();
        let mut done = false;
        let mut ok = true;
        for (lvl, msg) in logs {
            println!("{msg}");
            if msg.starts_with("[Pair]") {
                done = true;
                if lvl == LogLevel::Error {
                    ok = false;
                }
            }
        }
        if done {
            return if ok { ExitCode::SUCCESS } else { ExitCode::from(1) };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[Error] Timed out waiting for pairing response.");
    ExitCode::from(1)
}

fn run_cli_scan(opts: &CliOptions) -> ExitCode {
    println!("============================================================");
    println!(" QuestUTSync - Hands-Free Room Scan (No HTC App Required)");
    println!("============================================================");
    let scanner = RoomScanner::new(!opts.no_autolaunch, opts.dry_run, true);

    // Wait up to 8s for standalone ViveTrackerServer.exe to come online
    let start_wait = Instant::now();
    loop {
        for (_, msg) in scanner.drain_logs() {
            println!("{msg}");
        }
        let snap = scanner.snapshot();
        match snap.server_state {
            ServerState::Running => break,
            ServerState::NotInstalled => {
                eprintln!("[Error] ViveTrackerServer.exe not found.");
                return ExitCode::from(1);
            }
            ServerState::Offline if start_wait.elapsed() >= Duration::from_secs(14) => {
                eprintln!("[Error] ViveTrackerServer.exe did not start within 14s.");
                return ExitCode::from(1);
            }
            _ => std::thread::sleep(Duration::from_millis(150)),
        }
    }

    scanner.start_scan();
    let mut last_pct: Option<u8> = None;

    loop {
        for (_, msg) in scanner.drain_logs() {
            println!("{msg}");
        }
        let snap = scanner.snapshot();
        match snap.phase {
            ScanPhase::Scanning {
                elapsed_s,
                overall_pct,
                low_light,
                overspeed,
                ..
            } => {
                if last_pct != Some(overall_pct) {
                    last_pct = Some(overall_pct);
                    let mut flags = String::new();
                    if overspeed {
                        flags.push_str(" [TOO FAST]");
                    }
                    if low_light {
                        flags.push_str(" [LOW LIGHT]");
                    }
                    println!("[Scanning] Coverage: {overall_pct}% ({elapsed_s:.1}s){flags}");
                }
            }
            ScanPhase::Completed { .. } => return ExitCode::SUCCESS,
            ScanPhase::Error { .. } => return ExitCode::from(1),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn run_headless(opts: &CliOptions) -> ExitCode {
    println!("============================================================");
    println!(" QuestUTSync - Automatic Vive Ultimate Tracker Calibration");
    println!("============================================================");
    if opts.feet_back_cm != 0.0 {
        println!("[Config] Feet placed {:.1} cm behind the headset.", opts.feet_back_cm);
    }

    let mut engine = CalibrationEngine::new(
        opts.dry_run,
        opts.feet_back_cm,
        false,
        true,
        !opts.no_autolaunch,
    );
    if !engine.try_connect_vr(false) {
        eprintln!("Make sure SteamVR is running, then launch QuestUTSync again.");
        return ExitCode::from(1);
    }

    println!("\n[Ready] How to sync at any time in VR:");
    println!("  1. Stand comfortably & look straight ahead for 1 second (1st beep = Armed).");
    println!("  2. Lift your LEFT leg up ~10+ cm while keeping your right foot planted (double beep = Synced!).\n");

    loop {
        let synced = engine.tick(false);
        if synced && opts.run_once {
            return ExitCode::SUCCESS;
        }
        if engine.vr.is_none() {
            return ExitCode::SUCCESS;
        }
        let sleep_ms = if engine.latest_frame.ready_for_solver() {
            33
        } else {
            200
        };
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

struct QuestUTSyncApp {
    // Declared BEFORE `engine` so Drop destroys IVROverlay before VR_ShutdownInternal / FreeLibrary.
    overlay: Option<DashboardOverlay>,
    overlay_attempted: bool,
    engine: CalibrationEngine,
}

impl QuestUTSyncApp {
    fn new(cc: &eframe::CreationContext<'_>, opts: &CliOptions) -> Self {
        ui::apply_style(&cc.egui_ctx);
        let mut engine = CalibrationEngine::new(
            opts.dry_run,
            opts.feet_back_cm,
            true,
            false,
            !opts.no_autolaunch,
        );
        engine.try_connect_vr(false);
        let mut app = Self {
            engine,
            overlay: None,
            overlay_attempted: false,
        };
        app.ensure_overlay(cc.gl.as_ref());
        app
    }

    fn ensure_overlay(&mut self, gl: Option<&std::sync::Arc<eframe::glow::Context>>) {
        if self.engine.vr.is_none() {
            self.overlay = None;
            self.overlay_attempted = false;
            return;
        }
        if self.overlay.is_some() || self.overlay_attempted {
            return;
        }
        self.overlay_attempted = true;
        let Some(gl) = gl else { return };
        let Some(api) = self.engine.vr.as_ref().and_then(|v| v.overlay_api()) else {
            return;
        };
        match DashboardOverlay::try_create(api, gl.clone()) {
            Ok(ov) => {
                self.engine.log(
                    LogLevel::Success,
                    "[Overlay] Registered SteamVR Dashboard tab (QuestUTSync).",
                );
                self.overlay = Some(ov);
            }
            Err(e) => {
                self.engine
                    .log(LogLevel::Warn, format!("[Overlay] Disabled: {e}"));
            }
        }
    }
}

impl eframe::App for QuestUTSyncApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Destroy overlay BEFORE VR_ShutdownInternal if SteamVR is quitting.
        if self.engine.vr.as_ref().is_some_and(|v| v.poll_quit_requested()) {
            self.overlay = None;
            self.overlay_attempted = false;
            self.engine.disconnect_vr("SteamVR shutting down; disconnected.");
        }

        self.engine.tick(true);
        self.ensure_overlay(frame.gl());

        // Render shared UI to desktop window
        let overlay_ready = self.overlay.is_some();
        let actions = ui::draw(ctx, &mut self.engine, overlay_ready, false);
        if actions.reconnect_vr {
            self.overlay = None;
            self.overlay_attempted = false;
            self.engine.try_connect_vr(true);
            self.ensure_overlay(frame.gl());
        }
        if actions.show_in_dashboard {
            if let Some(ov) = self.overlay.as_ref() {
                ov.show_in_dashboard();
            }
        }

        // Render shared UI to SteamVR Dashboard Overlay when visible
        let mut overlay_visible = false;
        if let Some(mut ov) = self.overlay.take() {
            overlay_visible = ov.render_if_visible(|vr_ctx| {
                let _ = ui::draw(vr_ctx, &mut self.engine, true, true);
            });
            self.overlay = Some(ov);
        }

        // Keep polling at 60 Hz when VR overlay is visible, 30 Hz when tracking or scanning, 5 Hz when idle.
        let scanning_active = matches!(
            self.engine.scan_snapshot.phase,
            ScanPhase::Starting | ScanPhase::Scanning { .. } | ScanPhase::Finalizing { .. }
        );
        let interval_ms = if overlay_visible {
            16
        } else if self.engine.latest_frame.ready_for_solver() || scanning_active {
            33
        } else {
            200
        };
        ctx.request_repaint_after(Duration::from_millis(interval_ms));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Destroy overlay before dropping OpenVrSession (VR_ShutdownInternal).
        self.overlay = None;
        self.engine.vr = None;
    }
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&raw_args) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("{msg}\n");
            print_help();
            return ExitCode::from(2);
        }
    };

    if opts.show_help {
        print_help();
        return ExitCode::SUCCESS;
    }

    if opts.self_test {
        return match solver::run_self_test() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("[FAIL] {e}");
                ExitCode::from(1)
            }
        };
    }

    if !opts.dry_run {
        if let Some(code) = try_relaunch_with_allowed_pipe_name(&raw_args) {
            return code;
        }
    }

    if opts.pair_tracker {
        return run_cli_pair(&opts);
    }

    if opts.scan_room {
        return run_cli_scan(&opts);
    }

    if opts.headless {
        return run_headless(&opts);
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("QuestUTSync — Vive Ultimate Tracker Calibrator")
            .with_inner_size([860.0, 680.0])
            .with_min_inner_size([680.0, 520.0]),
        ..Default::default()
    };

    match eframe::run_native(
        "QuestUTSync",
        native_options,
        Box::new(move |cc| Ok(Box::new(QuestUTSyncApp::new(cc, &opts)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[GUI Error] {e}");
            ExitCode::from(1)
        }
    }
}
