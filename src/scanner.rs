use crate::detector::LogLevel;
use crate::pipe::{
    self, EnumeratedTracker, IndicatorEvent, MapScanEvent, MapState, PipeClient,
    SpaceCalibrationState, StatusIndicator, NTFY_PAIR_INFO, NTFY_SETUP_STATUS,
    NTFY_STATUS_INDICATOR, PIPE_NAME,
};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Diagnostics::Debug::Beep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Checking,
    Launching,
    Running,
    Offline,
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScanPhase {
    Idle,
    Starting,
    Scanning {
        elapsed_s: f32,
        host_serial: String,
        coverage: [u8; 8],
        overall_pct: u8,
        low_light: bool,
        overspeed: bool,
    },
    Finalizing {
        elapsed_s: f32,
        note: String,
    },
    Completed {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct ScanSnapshot {
    pub server_state: ServerState,
    pub launched_standalone: bool,
    pub tracker_count: u32,
    pub paired_trackers: Vec<EnumeratedTracker>,
    pub host_serial: Option<String>,
    pub map_state: MapState,
    pub cali_state: SpaceCalibrationState,
    pub phase: ScanPhase,
    pub auto_finish: bool,
    pub target_quality: u8,
    pub pairing_busy: bool,
    pub pair_status: Option<String>,
}

impl Default for ScanSnapshot {
    fn default() -> Self {
        Self {
            server_state: ServerState::Checking,
            launched_standalone: false,
            tracker_count: 0,
            paired_trackers: Vec::new(),
            host_serial: None,
            map_state: MapState::NoTrackerConnected,
            cali_state: SpaceCalibrationState::MapRequired,
            phase: ScanPhase::Idle,
            auto_finish: true,
            target_quality: 70,
            pairing_busy: false,
            pair_status: None,
        }
    }
}

struct SharedInner {
    snapshot: ScanSnapshot,
    pending_logs: VecDeque<(LogLevel, String)>,
}

enum ScanCommand {
    StartScan,
    FinishScan,
    CancelScan,
    LaunchServer,
    PairTracker(Option<u32>),
    UnpairSlot(u32),
    SetAudio(bool),
    SetAutoFinish(bool, u8),
    Shutdown,
}

pub struct RoomScanner {
    tx: Sender<ScanCommand>,
    shared: Arc<Mutex<SharedInner>>,
    pub auto_finish: bool,
    pub target_quality: u8,
    pub selected_slot: u32,
    last_audio: bool,
}

impl RoomScanner {
    pub fn new(auto_launch_server: bool, dry_run: bool, audio_beeps: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(SharedInner {
            snapshot: ScanSnapshot::default(),
            pending_logs: VecDeque::with_capacity(64),
        }));
        let worker_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            worker_loop(rx, worker_shared, auto_launch_server, dry_run, audio_beeps);
        });

        Self {
            tx,
            shared,
            auto_finish: true,
            target_quality: 70,
            selected_slot: 0,
            last_audio: audio_beeps,
        }
    }

    pub fn snapshot(&self) -> ScanSnapshot {
        self.shared
            .lock()
            .map(|g| g.snapshot.clone())
            .unwrap_or_default()
    }

    pub fn drain_logs(&self) -> Vec<(LogLevel, String)> {
        let Ok(mut g) = self.shared.lock() else {
            return Vec::new();
        };
        g.pending_logs.drain(..).collect()
    }

    pub fn sync_settings(&mut self, audio_beeps: bool) {
        if audio_beeps != self.last_audio {
            self.last_audio = audio_beeps;
            let _ = self.tx.send(ScanCommand::SetAudio(audio_beeps));
        }
        let _ = self
            .tx
            .send(ScanCommand::SetAutoFinish(self.auto_finish, self.target_quality));
    }

    pub fn start_scan(&self) {
        let _ = self
            .tx
            .send(ScanCommand::SetAutoFinish(self.auto_finish, self.target_quality));
        let _ = self.tx.send(ScanCommand::StartScan);
    }

    pub fn finish_scan(&self) {
        let _ = self.tx.send(ScanCommand::FinishScan);
    }

    pub fn cancel_scan(&self) {
        let _ = self.tx.send(ScanCommand::CancelScan);
    }

    pub fn launch_server(&self) {
        let _ = self.tx.send(ScanCommand::LaunchServer);
    }

    pub fn pair_tracker(&self, slot: Option<u32>) {
        let _ = self.tx.send(ScanCommand::PairTracker(slot));
    }

    pub fn unpair_slot(&self, slot: u32) {
        let _ = self.tx.send(ScanCommand::UnpairSlot(slot));
    }
}

impl Drop for RoomScanner {
    fn drop(&mut self) {
        let _ = self.tx.send(ScanCommand::Shutdown);
    }
}

/// Converts the 8-byte `MapCoverage` sector array into a 0..=100 percentage.
/// Handles both 0..100 per-sector quality values and 0/1 boolean sector flags.
pub fn compute_coverage_pct(sectors: [u8; 8]) -> u8 {
    let sum: u32 = sectors.iter().map(|&v| v as u32).sum();
    if sum == 0 {
        return 0;
    }
    if sectors.iter().all(|&v| v <= 1) {
        ((sum * 100) / 8).min(100) as u8
    } else {
        let clamped_sum: u32 = sectors.iter().map(|&v| v.min(100) as u32).sum();
        (clamped_sum / 8).min(100) as u8
    }
}

/// Normalizes a single sector byte to 0..=100 for UI rendering.
pub fn normalize_sector_pct(sectors: [u8; 8], idx: usize) -> u8 {
    let v = sectors[idx];
    if sectors.iter().all(|&x| x <= 1) {
        if v > 0 { 100 } else { 0 }
    } else {
        v.min(100)
    }
}

struct ActiveScan {
    started_at: Instant,
    host_handle: u64,
    host_serial: String,
    coverage: [u8; 8],
    overall_pct: u8,
    low_light: bool,
    overspeed: bool,
    hi_sent: bool,
    last_milestone: u8,
    last_warn_beep: Instant,
    last_poll: Instant,
}

struct FinalizingScan {
    started_at: Instant,
    last_poll: Instant,
}

fn push_log(shared: &Arc<Mutex<SharedInner>>, level: LogLevel, msg: impl Into<String>) {
    if let Ok(mut g) = shared.lock() {
        if g.pending_logs.len() >= 64 {
            g.pending_logs.pop_front();
        }
        g.pending_logs.push_back((level, msg.into()));
    }
}

fn update_snapshot(shared: &Arc<Mutex<SharedInner>>, f: impl FnOnce(&mut ScanSnapshot)) {
    if let Ok(mut g) = shared.lock() {
        f(&mut g.snapshot);
    }
}

fn play_tones(audio: bool, tones: &[(u32, u32)]) {
    if !audio {
        return;
    }
    let seq = tones.to_vec();
    std::thread::spawn(move || unsafe {
        for (freq, ms) in seq {
            if freq > 0 && ms > 0 {
                Beep(freq, ms);
            }
        }
    });
}

fn ensure_server_and_connect(
    shared: &Arc<Mutex<SharedInner>>,
    allow_launch: bool,
    dry_run: bool,
) -> Option<PipeClient> {
    if pipe::is_pipe_available(PIPE_NAME) {
        if let Ok(client) = PipeClient::connect(PIPE_NAME, 600) {
            update_snapshot(shared, |s| s.server_state = ServerState::Running);
            return Some(client);
        }
    }

    if !allow_launch || dry_run {
        let installed = pipe::find_vive_tracker_server_exe().is_some();
        update_snapshot(shared, |s| {
            s.server_state = if installed {
                ServerState::Offline
            } else {
                ServerState::NotInstalled
            };
        });
        return None;
    }

    if pipe::find_vive_tracker_server_exe().is_none() {
        update_snapshot(shared, |s| s.server_state = ServerState::NotInstalled);
        push_log(
            shared,
            LogLevel::Warn,
            "[VIVE Server] ViveTrackerServer.exe not found (install VIVE Hub runtime first).",
        );
        return None;
    }

    update_snapshot(shared, |s| s.server_state = ServerState::Launching);
    match pipe::launch_vive_tracker_server_silent() {
        Ok(exe) => {
            push_log(
                shared,
                LogLevel::Accent,
                format!(
                    "[VIVE Server] Launched {} silently (standalone, no VIVE Hub UI).",
                    exe.file_name().unwrap_or_default().to_string_lossy()
                ),
            );
        }
        Err(e) => {
            update_snapshot(shared, |s| s.server_state = ServerState::Offline);
            push_log(shared, LogLevel::Error, format!("[VIVE Server] {e}"));
            return None;
        }
    }

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if pipe::is_pipe_available(PIPE_NAME) {
            if let Ok(client) = PipeClient::connect(PIPE_NAME, 500) {
                update_snapshot(shared, |s| {
                    s.server_state = ServerState::Running;
                    s.launched_standalone = true;
                });
                push_log(
                    shared,
                    LogLevel::Success,
                    "[VIVE Server] Standalone tracker backend ready on TrackerNamedPipe.",
                );
                return Some(client);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    update_snapshot(shared, |s| s.server_state = ServerState::Offline);
    push_log(
        shared,
        LogLevel::Warn,
        "[VIVE Server] Launched ViveTrackerServer.exe but TrackerNamedPipe did not open within 12s.",
    );
    None
}

fn apply_indicator(scan: &mut ActiveScan, ind: StatusIndicator) {
    match ind.event_index {
        0 => {
            scan.coverage = ind.map_coverage;
            scan.overall_pct = compute_coverage_pct(scan.coverage);
        }
        1 => {
            // TrackerIndicatorLight: 1 = Normal, 2 = LowLight
            scan.low_light = ind.light_state == 2;
        }
        2 => {
            // MovementOverspeed: non-zero / 2 indicates moving too fast
            scan.overspeed = ind.light_state != 0 && ind.light_state != 1;
        }
        _ => {}
    }
}

fn handle_ntfy(
    cmd: u32,
    payload: &[u8],
    shared: &Arc<Mutex<SharedInner>>,
    active: &mut Option<ActiveScan>,
) {
    match cmd {
        NTFY_PAIR_INFO => {
            // NotifyPairInfo_return: EnumerateTrackerItem (40B) + tracker_state (4B) + notify_type (4B) + tracker_count (4B)
            if payload.len() >= 52 {
                let nul = payload[0..32].iter().position(|&b| b == 0).unwrap_or(32);
                let serial = String::from_utf8_lossy(&payload[0..nul]).trim().to_string();
                let notify_type = u32::from_le_bytes(payload[44..48].try_into().unwrap());
                let count = u32::from_le_bytes(payload[48..52].try_into().unwrap());
                let verb = if notify_type == 1 { "connected" } else { "disconnected" };
                let msg = if serial.is_empty() {
                    format!("Tracker {verb} ({count} paired).")
                } else {
                    format!("Tracker {serial} {verb} ({count} total).")
                };
                push_log(shared, LogLevel::Accent, format!("[Pair] {msg}"));
                update_snapshot(shared, |s| {
                    s.tracker_count = count;
                    s.pairing_busy = false;
                    s.pair_status = Some(msg);
                });
            }
        }
        NTFY_SETUP_STATUS => {
            if let Ok(st) = pipe::parse_setup_status_payload(payload) {
                update_snapshot(shared, |s| {
                    s.map_state = st.map_state;
                    s.cali_state = st.cali_state;
                });
            }
        }
        NTFY_STATUS_INDICATOR => {
            if let (Some(scan), Some(ind)) = (
                active.as_mut(),
                pipe::parse_status_indicator_payload(payload),
            ) {
                apply_indicator(scan, ind);
            }
        }
        _ => {}
    }
}

fn worker_loop(
    rx: Receiver<ScanCommand>,
    shared: Arc<Mutex<SharedInner>>,
    auto_launch_server: bool,
    dry_run: bool,
    mut audio_beeps: bool,
) {
    let mut auto_finish = true;
    let mut target_quality: u8 = 70;
    let mut client: Option<PipeClient> =
        ensure_server_and_connect(&shared, auto_launch_server, dry_run);
    let mut active_scan: Option<ActiveScan> = None;
    let mut finalizing: Option<FinalizingScan> = None;
    let mut last_idle_poll = Instant::now() - Duration::from_secs(10);
    let mut last_reconnect = Instant::now();

    loop {
        let wait_ms = if active_scan.is_some() {
            60
        } else if finalizing.is_some() {
            120
        } else {
            180
        };

        let cmd = match rx.recv_timeout(Duration::from_millis(wait_ms)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if let Some(c) = cmd {
            match c {
                ScanCommand::Shutdown => {
                    if active_scan.is_some() {
                        if let Some(cl) = client.as_mut() {
                            let _ = cl.cancel_map_creation(&mut |_, _| {});
                        }
                    }
                    break;
                }
                ScanCommand::SetAudio(v) => audio_beeps = v,
                ScanCommand::SetAutoFinish(af, tq) => {
                    auto_finish = af;
                    target_quality = tq.clamp(40, 95);
                    update_snapshot(&shared, |s| {
                        s.auto_finish = auto_finish;
                        s.target_quality = target_quality;
                    });
                }
                ScanCommand::LaunchServer => {
                    client = ensure_server_and_connect(&shared, true, dry_run);
                    last_idle_poll = Instant::now() - Duration::from_secs(10);
                }
                ScanCommand::PairTracker(opt_slot) => {
                    if dry_run {
                        push_log(
                            &shared,
                            LogLevel::Info,
                            "[Pair] Dry-run mode active; skipping PAIR_TRACKER_WITH_DONGLE.",
                        );
                        continue;
                    }
                    if client.as_ref().is_none_or(|c| c.is_disconnected()) {
                        client = ensure_server_and_connect(&shared, true, dry_run);
                    }
                    let Some(cl) = client.as_mut() else {
                        let msg = "ViveTrackerServer.exe is not running; cannot pair tracker.".to_string();
                        push_log(&shared, LogLevel::Error, format!("[Pair] {msg}"));
                        update_snapshot(&shared, |s| s.pair_status = Some(msg));
                        continue;
                    };

                    // Auto-select first free dongle slot (0..=4) if caller didn't pick one.
                    let slot = opt_slot.unwrap_or_else(|| {
                        let snap = shared.lock().map(|g| g.snapshot.clone()).unwrap_or_default();
                        let used: Vec<u32> = snap
                            .paired_trackers
                            .iter()
                            .filter_map(|t| t.display_num.checked_sub(1))
                            .collect();
                        (0..5)
                            .find(|s| !used.contains(s))
                            .unwrap_or_else(|| snap.tracker_count.min(4))
                    });

                    update_snapshot(&shared, |s| {
                        s.pairing_busy = true;
                        s.pair_status = Some(format!(
                            "Entering pairing mode on Dongle Slot #{}...",
                            slot + 1
                        ));
                    });
                    let mut none_active = None;
                    match cl.pair_tracker_with_dongle(slot, &mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut none_active)
                    }) {
                        Ok(()) => {
                            let msg = format!(
                                "Dongle Slot #{} pairing active — hold tracker power button until LED blinks blue.",
                                slot + 1
                            );
                            push_log(&shared, LogLevel::Accent, format!("[Pair] {msg}"));
                            play_tones(audio_beeps, &[(600, 90), (900, 120)]);
                            update_snapshot(&shared, |s| {
                                s.pairing_busy = false;
                                s.pair_status = Some(msg);
                            });
                            last_idle_poll = Instant::now() - Duration::from_secs(10);
                        }
                        Err(e) => {
                            let msg = format!(
                                "Pairing failed on Slot #{} ({e}). Make sure the VIVE Wireless Dongle is plugged in.",
                                slot + 1
                            );
                            push_log(&shared, LogLevel::Error, format!("[Pair] {msg}"));
                            play_tones(audio_beeps, &[(360, 220)]);
                            update_snapshot(&shared, |s| {
                                s.pairing_busy = false;
                                s.pair_status = Some(msg);
                            });
                        }
                    }
                }
                ScanCommand::UnpairSlot(slot) => {
                    if dry_run {
                        push_log(
                            &shared,
                            LogLevel::Info,
                            "[Pair] Dry-run mode active; skipping UNPAIR_TRACKER_WITH_DONGLE.",
                        );
                        continue;
                    }
                    if client.as_ref().is_none_or(|c| c.is_disconnected()) {
                        client = ensure_server_and_connect(&shared, true, dry_run);
                    }
                    let Some(cl) = client.as_mut() else {
                        let msg = "ViveTrackerServer.exe is not running; cannot unpair.".to_string();
                        push_log(&shared, LogLevel::Error, format!("[Pair] {msg}"));
                        update_snapshot(&shared, |s| s.pair_status = Some(msg));
                        continue;
                    };
                    let mut none_active = None;
                    match cl.unpair_tracker_with_dongle(slot, &mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut none_active)
                    }) {
                        Ok(()) => {
                            let msg = format!("Unpaired Dongle Slot #{}.", slot + 1);
                            push_log(&shared, LogLevel::Warn, format!("[Pair] {msg}"));
                            play_tones(audio_beeps, &[(440, 100)]);
                            update_snapshot(&shared, |s| s.pair_status = Some(msg));
                            last_idle_poll = Instant::now() - Duration::from_secs(10);
                        }
                        Err(e) => {
                            let msg = format!("Unpair Slot #{} failed ({e}).", slot + 1);
                            push_log(&shared, LogLevel::Error, format!("[Pair] {msg}"));
                            update_snapshot(&shared, |s| s.pair_status = Some(msg));
                        }
                    }
                }
                ScanCommand::StartScan => {
                    if active_scan.is_some() || finalizing.is_some() {
                        continue;
                    }
                    if dry_run {
                        push_log(
                            &shared,
                            LogLevel::Info,
                            "[Room Scan] Dry-run mode active; skipping Start_Map_Creation.",
                        );
                        continue;
                    }
                    update_snapshot(&shared, |s| s.phase = ScanPhase::Starting);
                    if client.as_ref().is_none_or(|c| c.is_disconnected()) {
                        client = ensure_server_and_connect(&shared, true, dry_run);
                    }
                    let Some(cl) = client.as_mut() else {
                        let msg =
                            "ViveTrackerServer.exe is not running; cannot start room scan."
                                .to_string();
                        push_log(&shared, LogLevel::Error, format!("[Room Scan] {msg}"));
                        update_snapshot(&shared, |s| s.phase = ScanPhase::Error { message: msg });
                        continue;
                    };

                    let mut dummy_active = None;
                    let host = match cl.get_host_handle(&mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut dummy_active)
                    }) {
                        Ok(h) => h,
                        Err(e) => {
                            let msg = format!(
                                "No host tracker ready ({e}). Turn on at least 1 VIVE Ultimate Tracker first."
                            );
                            push_log(&shared, LogLevel::Warn, format!("[Room Scan] {msg}"));
                            play_tones(audio_beeps, &[(360, 220)]);
                            update_snapshot(&shared, |s| {
                                s.phase = ScanPhase::Error { message: msg }
                            });
                            continue;
                        }
                    };

                    if let Err(e) = cl.start_map_creation(&mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut dummy_active)
                    }) {
                        let msg = format!("Start_Map_Creation failed: {e}");
                        push_log(&shared, LogLevel::Error, format!("[Room Scan] {msg}"));
                        play_tones(audio_beeps, &[(360, 240)]);
                        update_snapshot(&shared, |s| s.phase = ScanPhase::Error { message: msg });
                        continue;
                    }

                    // Notify firmware to initialize scan center & begin low-level coverage tracking.
                    let _ = cl.send_map_scan_event(MapScanEvent::SetCenter, 0, &mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut dummy_active)
                    });
                    let _ = cl.send_map_scan_event(MapScanEvent::LoScanStarted, 0, &mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut dummy_active)
                    });

                    let serial_label = if host.serial.is_empty() {
                        format!("Handle 0x{:X}", host.handle)
                    } else {
                        host.serial.clone()
                    };
                    push_log(
                        &shared,
                        LogLevel::Accent,
                        format!(
                            "[Room Scan] Started on tracker {serial_label}. Slowly pan/turn 360° around your room (no headset needed)..."
                        ),
                    );
                    play_tones(audio_beeps, &[(520, 90), (780, 120)]);

                    let now = Instant::now();
                    active_scan = Some(ActiveScan {
                        started_at: now,
                        host_handle: host.handle,
                        host_serial: serial_label.clone(),
                        coverage: [0; 8],
                        overall_pct: 0,
                        low_light: false,
                        overspeed: false,
                        hi_sent: false,
                        last_milestone: 0,
                        last_warn_beep: now - Duration::from_secs(10),
                        last_poll: now - Duration::from_secs(1),
                    });
                    update_snapshot(&shared, |s| {
                        s.host_serial = Some(serial_label.clone());
                        s.phase = ScanPhase::Scanning {
                            elapsed_s: 0.0,
                            host_serial: serial_label,
                            coverage: [0; 8],
                            overall_pct: 0,
                            low_light: false,
                            overspeed: false,
                        };
                    });
                }
                ScanCommand::FinishScan => {
                    if let Some(scan) = active_scan.take() {
                        begin_finalize(
                            &mut client,
                            &shared,
                            &mut finalizing,
                            scan.overall_pct,
                            audio_beeps,
                        );
                    }
                }
                ScanCommand::CancelScan => {
                    let was_active = active_scan.take().is_some() || finalizing.take().is_some();
                    if was_active {
                        if let Some(cl) = client.as_mut() {
                            let _ = cl.cancel_map_creation(&mut |_, _| {});
                        }
                        push_log(&shared, LogLevel::Warn, "[Room Scan] Scan cancelled.");
                        play_tones(audio_beeps, &[(380, 150)]);
                        update_snapshot(&shared, |s| s.phase = ScanPhase::Idle);
                    }
                }
            }
        }

        // Handle active scan polling & auto-finish
        if let Some(mut scan) = active_scan.take() {
            let mut should_finish = false;
            let mut scan_failed: Option<String> = None;

            if let Some(cl) = client.as_mut() {
                let mut temp = Some(scan);
                cl.drain_notifications(20, &mut |c, p| handle_ntfy(c, p, &shared, &mut temp));
                scan = temp.unwrap();

                let elapsed = scan.started_at.elapsed().as_secs_f32();
                if !scan.hi_sent && (scan.overall_pct >= 40 || elapsed >= 6.0) {
                    scan.hi_sent = true;
                    let mut temp = Some(scan);
                    let _ = cl.send_map_scan_event(MapScanEvent::HiScanStarted, 0, &mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut temp)
                    });
                    scan = temp.unwrap();
                }

                if scan.last_poll.elapsed() >= Duration::from_millis(250) {
                    scan.last_poll = Instant::now();
                    for ev in [
                        IndicatorEvent::MapCoverage,
                        IndicatorEvent::EnvironmentLight,
                        IndicatorEvent::MovementOverspeed,
                    ] {
                        let mut temp = Some(scan);
                        let res = cl.get_device_indicator(
                            temp.as_ref().unwrap().host_handle,
                            ev,
                            &mut |c, p| handle_ntfy(c, p, &shared, &mut temp),
                        );
                        scan = temp.unwrap();
                        match res {
                            Ok(ind) => apply_indicator(&mut scan, ind),
                            Err(_) if cl.is_disconnected() => {
                                scan_failed =
                                    Some("Lost connection to ViveTrackerServer during scan.".into());
                                break;
                            }
                            Err(_) => {}
                        }
                    }
                }

                // Rising audio milestone ticks every 15%
                let bucket = (scan.overall_pct / 15) * 15;
                if bucket > scan.last_milestone && bucket > 0 {
                    scan.last_milestone = bucket;
                    let freq = 440 + (bucket as u32) * 8;
                    play_tones(audio_beeps, &[(freq, 70)]);
                } else if (scan.overspeed || scan.low_light)
                    && scan.last_warn_beep.elapsed() >= Duration::from_millis(2500)
                {
                    scan.last_warn_beep = Instant::now();
                    play_tones(audio_beeps, &[(300, 65), (250, 65)]);
                }

                if auto_finish && scan.overall_pct >= target_quality && elapsed >= 5.0 {
                    should_finish = true;
                }
            } else {
                scan_failed = Some("Pipe disconnected during scan.".into());
            }

            if let Some(err) = scan_failed {
                push_log(&shared, LogLevel::Error, format!("[Room Scan] {err}"));
                play_tones(audio_beeps, &[(340, 260)]);
                update_snapshot(&shared, |s| s.phase = ScanPhase::Error { message: err });
            } else if should_finish {
                push_log(
                    &shared,
                    LogLevel::Success,
                    format!(
                        "[Room Scan] Target coverage reached ({}% >= {}%). Auto-finishing & syncing map...",
                        scan.overall_pct, target_quality
                    ),
                );
                begin_finalize(
                    &mut client,
                    &shared,
                    &mut finalizing,
                    scan.overall_pct,
                    audio_beeps,
                );
            } else {
                let elapsed = scan.started_at.elapsed().as_secs_f32();
                update_snapshot(&shared, |s| {
                    s.phase = ScanPhase::Scanning {
                        elapsed_s: elapsed,
                        host_serial: scan.host_serial.clone(),
                        coverage: scan.coverage,
                        overall_pct: scan.overall_pct,
                        low_light: scan.low_light,
                        overspeed: scan.overspeed,
                    };
                });
                active_scan = Some(scan);
            }
            continue;
        }

        // Handle finalizing/syncing map wait
        if let Some(mut fin) = finalizing.take() {
            let elapsed = fin.started_at.elapsed().as_secs_f32();
            let mut done_msg: Option<String> = None;

            if fin.last_poll.elapsed() >= Duration::from_millis(500) {
                fin.last_poll = Instant::now();
                if let Some(cl) = client.as_mut() {
                    let mut none_active = None;
                    if let Ok(st) = cl.get_setup_status(&mut |c, p| {
                        handle_ntfy(c, p, &shared, &mut none_active)
                    }) {
                        update_snapshot(&shared, |s| {
                            s.map_state = st.map_state;
                            s.cali_state = st.cali_state;
                        });
                        if st.map_state == MapState::Ready {
                            done_msg = Some(format!(
                                "Room map saved & synced in {:.1}s! Stand still + lift LEFT foot to calibrate.",
                                elapsed
                            ));
                        }
                    }
                }
            }

            if let Some(msg) = done_msg {
                push_log(&shared, LogLevel::Success, format!("[Room Scan] {msg}"));
                play_tones(audio_beeps, &[(660, 90), (880, 90), (1320, 180)]);
                update_snapshot(&shared, |s| s.phase = ScanPhase::Completed { message: msg });
            } else if elapsed >= 35.0 {
                let msg =
                    "Map save requested; trackers may still be syncing in the background."
                        .to_string();
                push_log(&shared, LogLevel::Warn, format!("[Room Scan] {msg}"));
                play_tones(audio_beeps, &[(660, 100), (880, 120)]);
                update_snapshot(&shared, |s| s.phase = ScanPhase::Completed { message: msg });
            } else {
                update_snapshot(&shared, |s| {
                    s.phase = ScanPhase::Finalizing {
                        elapsed_s: elapsed,
                        note: "Downloading, refining & syncing map to all trackers...".to_string(),
                    };
                });
                finalizing = Some(fin);
            }
            continue;
        }

        // Idle: maintain pipe connection & poll setup status every 1.5s
        if client.as_ref().is_none_or(|c| c.is_disconnected()) {
            client = None;
            if last_reconnect.elapsed() >= Duration::from_secs(2) {
                last_reconnect = Instant::now();
                if pipe::is_pipe_available(PIPE_NAME) {
                    if let Ok(cl) = PipeClient::connect(PIPE_NAME, 300) {
                        client = Some(cl);
                        update_snapshot(&shared, |s| s.server_state = ServerState::Running);
                    }
                } else {
                    let installed = pipe::find_vive_tracker_server_exe().is_some();
                    update_snapshot(&shared, |s| {
                        s.server_state = if installed {
                            ServerState::Offline
                        } else {
                            ServerState::NotInstalled
                        };
                    });
                }
            }
            continue;
        }

        if last_idle_poll.elapsed() >= Duration::from_millis(1500) {
            last_idle_poll = Instant::now();
            if let Some(cl) = client.as_mut() {
                let mut none_active = None;
                if let Ok(st) = cl.get_setup_status(&mut |c, p| {
                    handle_ntfy(c, p, &shared, &mut none_active)
                }) {
                    update_snapshot(&shared, |s| {
                        s.server_state = ServerState::Running;
                        s.map_state = st.map_state;
                        s.cali_state = st.cali_state;
                    });
                }
                if let Ok(cnt) = cl.get_tracker_count(&mut |c, p| {
                    handle_ntfy(c, p, &shared, &mut none_active)
                }) {
                    update_snapshot(&shared, |s| s.tracker_count = cnt);
                }
                if let Ok(list) = cl.enumerate_trackers(&mut |c, p| {
                    handle_ntfy(c, p, &shared, &mut none_active)
                }) {
                    update_snapshot(&shared, |s| s.paired_trackers = list);
                }
                match cl.get_host_handle(&mut |c, p| {
                    handle_ntfy(c, p, &shared, &mut none_active)
                }) {
                    Ok(h) if !h.serial.is_empty() => {
                        update_snapshot(&shared, |s| s.host_serial = Some(h.serial));
                    }
                    _ => {}
                }
                if cl.is_disconnected() {
                    client = None;
                }
            }
        }
    }
}

fn begin_finalize(
    client: &mut Option<PipeClient>,
    shared: &Arc<Mutex<SharedInner>>,
    finalizing: &mut Option<FinalizingScan>,
    pct: u8,
    audio_beeps: bool,
) {
    let Some(cl) = client.as_mut() else {
        update_snapshot(shared, |s| {
            s.phase = ScanPhase::Error {
                message: "Pipe disconnected before map could be saved.".to_string(),
            }
        });
        return;
    };
    let mut none_active = None;
    let _ = cl.send_map_scan_event(MapScanEvent::ScanEnd, 0, &mut |c, p| {
        handle_ntfy(c, p, shared, &mut none_active)
    });
    match cl.end_map_creation(&mut |c, p| handle_ntfy(c, p, shared, &mut none_active)) {
        Ok(()) => {
            push_log(
                shared,
                LogLevel::Accent,
                format!("[Room Scan] End_Map_Creation sent (coverage {pct}%). Waiting for map refinement & sync..."),
            );
            play_tones(audio_beeps, &[(780, 90), (1040, 110)]);
            let now = Instant::now();
            *finalizing = Some(FinalizingScan {
                started_at: now,
                last_poll: now,
            });
            update_snapshot(shared, |s| {
                s.phase = ScanPhase::Finalizing {
                    elapsed_s: 0.0,
                    note: "Downloading, refining & syncing map to all trackers...".to_string(),
                };
            });
        }
        Err(e) => {
            let msg = format!("End_Map_Creation failed: {e}");
            push_log(shared, LogLevel::Error, format!("[Room Scan] {msg}"));
            play_tones(audio_beeps, &[(340, 260)]);
            update_snapshot(shared, |s| s.phase = ScanPhase::Error { message: msg });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_percentage_handles_percent_and_binary_sectors() {
        assert_eq!(compute_coverage_pct([0; 8]), 0);
        assert_eq!(compute_coverage_pct([1, 1, 1, 1, 0, 0, 0, 0]), 50);
        assert_eq!(compute_coverage_pct([1; 8]), 100);
        assert_eq!(compute_coverage_pct([80, 80, 60, 60, 80, 80, 60, 60]), 70);
        assert_eq!(compute_coverage_pct([120, 100, 100, 100, 100, 100, 100, 100]), 100);
        assert_eq!(normalize_sector_pct([1, 0, 1, 0, 0, 0, 0, 0], 0), 100);
        assert_eq!(normalize_sector_pct([1, 0, 1, 0, 0, 0, 0, 0], 1), 0);
        assert_eq!(normalize_sector_pct([75, 40, 0, 0, 0, 0, 0, 0], 0), 75);
    }
}
