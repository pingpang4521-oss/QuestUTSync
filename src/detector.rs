use crate::openvr::{FrameSnapshot, OpenVrSession};
use crate::pipe::{self, SendOutcome, PIPE_NAME};
use crate::solver::{self, CalibrationTransform, DeltaTransform, StandingIndices};
use glam::{Vec2, Vec3};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::System::Diagnostics::Debug::Beep;

const STILL_VELOCITY_THRESHOLD: f32 = 0.04;
const STILL_ANGULAR_THRESHOLD: f32 = 0.12;
const STILL_DURATION_REQUIRED: f32 = 1.0;
const ARMED_TIMEOUT_SECONDS: f32 = 4.5;
const POSE_DROP_GRACE_SECONDS: f32 = 0.30;
const LIFT_HEIGHT_THRESHOLD: f32 = 0.10;
const PLANTED_MAX_MOVE: f32 = 0.035;
const HEAD_MAX_MOVE_DURING_LIFT: f32 = 0.08;
const WAIST_MAX_DROP_DURING_LIFT: f32 = 0.10;
const COOLDOWN_SECONDS: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Success,
    Accent,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub elapsed_s: f32,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum DetectorPhase {
    Disconnected,
    Paused,
    WaitingForDevices,
    StandStill,
    HoldingStill {
        progress: f32,
    },
    Armed {
        remaining_s: f32,
        max_rise_cm: f32,
    },
    Cooldown {
        remaining_s: f32,
        last_status: SyncResultKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResultKind {
    Synced,
    Unconfirmed,
    DryRun,
    Failed,
}

#[derive(Clone, Copy)]
struct StandingBaseline {
    hmd_pos: Vec3,
    hmd_forward_flat: Vec3,
    tracker_pos: [Vec3; 3],
    indices: StandingIndices,
}

pub struct CalibrationEngine {
    pub vr: Option<OpenVrSession>,
    pub vr_error: Option<String>,
    pub auto_sync: bool,
    pub dry_run: bool,
    pub feet_back_cm: f32,
    pub audio_beeps: bool,

    pub latest_frame: FrameSnapshot,
    pub phase: DetectorPhase,
    pub assigned_roles: HashMap<String, &'static str>,
    pub active_cal: CalibrationTransform,
    pub editor_pos: [f32; 3],
    pub editor_ypr: [f32; 3],
    pub editor_dirty: bool,
    pub last_delta: Option<DeltaTransform>,
    pub last_sync_kind: Option<SyncResultKind>,
    pub sync_count: u32,
    pub logs: VecDeque<LogEntry>,

    start_time: Instant,
    calib_path: PathBuf,
    last_sent_cal: Option<CalibrationTransform>,
    last_cal_file_mtime: Option<SystemTime>,
    last_reconnect_attempt: Instant,

    // Detector temporal state
    has_prev: bool,
    prev_hmd_pos: Vec3,
    prev_trk_pos: [Vec3; 3],
    last_tick: Instant,
    last_valid_pose_tick: Instant,
    still_timer: f32,
    still_sample_count: u32,
    still_jitter_frames: u32,
    sum_hmd_pos: Vec3,
    sum_hmd_forward: Vec3,
    sum_trk_pos: [Vec3; 3],
    baseline: Option<StandingBaseline>,
    armed_timer: f32,
    cooldown_until: Option<Instant>,
    last_device_count_log: isize,
    want_overlay: bool,
    echo_stdout: bool,
}

impl CalibrationEngine {
    pub fn new(dry_run: bool, feet_back_cm: f32, want_overlay: bool, echo_stdout: bool) -> Self {
        let calib_path = pipe::default_vive_calib_path();
        let (initial_cal, warn) = pipe::read_current_calibration(&calib_path);
        let (y, p, r) = initial_cal.to_ypr_degrees();
        let now = Instant::now();

        let mut engine = Self {
            vr: None,
            vr_error: None,
            auto_sync: true,
            dry_run,
            feet_back_cm,
            audio_beeps: true,
            latest_frame: FrameSnapshot::default(),
            phase: DetectorPhase::Disconnected,
            assigned_roles: HashMap::new(),
            active_cal: initial_cal,
            editor_pos: initial_cal.position.to_array(),
            editor_ypr: [y, p, r],
            editor_dirty: false,
            last_delta: None,
            last_sync_kind: None,
            sync_count: 0,
            logs: VecDeque::with_capacity(200),
            start_time: now,
            calib_path,
            last_sent_cal: None,
            last_cal_file_mtime: None,
            last_reconnect_attempt: now - Duration::from_secs(10),
            has_prev: false,
            prev_hmd_pos: Vec3::ZERO,
            prev_trk_pos: [Vec3::ZERO; 3],
            last_tick: now,
            last_valid_pose_tick: now,
            still_timer: 0.0,
            still_sample_count: 0,
            still_jitter_frames: 0,
            sum_hmd_pos: Vec3::ZERO,
            sum_hmd_forward: Vec3::ZERO,
            sum_trk_pos: [Vec3::ZERO; 3],
            baseline: None,
            armed_timer: 0.0,
            cooldown_until: None,
            last_device_count_log: -1,
            want_overlay,
            echo_stdout,
        };

        if let Some(w) = warn {
            engine.log(LogLevel::Warn, format!("[Calibration.json] {w}"));
        }
        engine
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        let msg = message.into();
        if self.echo_stdout {
            println!("{msg}");
        }
        let elapsed_s = self.start_time.elapsed().as_secs_f32();
        if self.logs.len() >= 150 {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            elapsed_s,
            level,
            message: msg,
        });
    }

    pub fn try_connect_vr(&mut self, launch_if_closed: bool) -> bool {
        self.last_reconnect_attempt = Instant::now();
        match OpenVrSession::try_init(self.want_overlay, launch_if_closed) {
            Ok(session) => {
                let has_ov = session.overlay_api().is_some();
                self.vr = Some(session);
                self.vr_error = None;
                self.phase = DetectorPhase::WaitingForDevices;
                self.last_device_count_log = -1;
                self.log(
                    LogLevel::Success,
                    if has_ov {
                        "[OpenVR] Connected to SteamVR (Dashboard Overlay enabled)."
                    } else {
                        "[OpenVR] Connected to SteamVR (Background mode)."
                    },
                );
                true
            }
            Err(e) => {
                let changed = self.vr_error.as_deref() != Some(e.as_str());
                self.vr_error = Some(e.clone());
                self.phase = DetectorPhase::Disconnected;
                if changed {
                    self.log(LogLevel::Warn, format!("[OpenVR] {e}"));
                }
                false
            }
        }
    }

    pub fn reload_calibration_from_disk(&mut self) {
        let (cal, warn) = pipe::read_current_calibration(&self.calib_path);
        if let Some(w) = warn {
            self.log(LogLevel::Warn, format!("[Calibration.json] {w}"));
        }
        self.set_active_calibration(cal);
        self.last_sent_cal = None;
        self.log(
            LogLevel::Info,
            format!(
                "[Calibration.json] Loaded Pos=({:.3}, {:.3}, {:.3})",
                cal.position.x, cal.position.y, cal.position.z
            ),
        );
    }

    pub fn set_active_calibration(&mut self, cal: CalibrationTransform) {
        self.active_cal = cal;
        self.editor_pos = cal.position.to_array();
        let (y, p, r) = cal.to_ypr_degrees();
        self.editor_ypr = [y, p, r];
        self.editor_dirty = false;
    }

    pub fn apply_editor_calibration(&mut self) {
        let cal = CalibrationTransform::from_ypr_degrees(
            Vec3::from_array(self.editor_pos),
            self.editor_ypr[0],
            self.editor_ypr[1],
            self.editor_ypr[2],
            self.active_cal.scale,
        );
        self.send_manual_calibration(cal, "Manual transform");
    }

    pub fn reset_calibration_to_identity(&mut self) {
        self.send_manual_calibration(CalibrationTransform::IDENTITY, "Identity reset");
    }

    fn send_manual_calibration(&mut self, cal: CalibrationTransform, label: &str) {
        self.set_active_calibration(cal);
        if self.dry_run {
            self.log(
                LogLevel::Info,
                format!("[Dry Run] {label} computed (not sent to pipe)."),
            );
            self.last_sync_kind = Some(SyncResultKind::DryRun);
            return;
        }
        match pipe::try_send_calibration(PIPE_NAME, cal) {
            SendOutcome::Acked(msg) => {
                self.last_sent_cal = Some(cal);
                self.last_cal_file_mtime = pipe::file_mtime(&self.calib_path);
                self.last_sync_kind = Some(SyncResultKind::Synced);
                self.log(LogLevel::Success, format!("[{label}] {msg}"));
                self.beep(880, 90, 1320, 120);
            }
            SendOutcome::Unconfirmed(msg) => {
                self.last_sent_cal = None;
                self.last_sync_kind = Some(SyncResultKind::Unconfirmed);
                self.log(LogLevel::Warn, format!("[Unconfirmed] {msg}"));
                self.beep(440, 300, 0, 0);
            }
            SendOutcome::Failed(msg) => {
                self.last_sync_kind = Some(SyncResultKind::Failed);
                self.log(LogLevel::Error, format!("[Pipe Warning] {msg}"));
            }
        }
    }

    pub fn disconnect_vr(&mut self, reason: &str) {
        self.log(LogLevel::Warn, format!("[OpenVR] {reason}"));
        self.vr = None;
        self.latest_frame = FrameSnapshot::default();
        self.disarm();
        self.phase = DetectorPhase::Disconnected;
    }

    pub fn disarm(&mut self) {
        self.baseline = None;
        self.armed_timer = 0.0;
        self.cooldown_until = None;
        self.reset_still();
        self.has_prev = false;
    }

    /// Advances one simulation step. Returns `true` if a sync just fired (useful for `--once`).
    pub fn tick(&mut self, allow_auto_reconnect: bool) -> bool {
        let now = Instant::now();

        if self.vr.is_none() {
            self.phase = DetectorPhase::Disconnected;
            if allow_auto_reconnect && now.duration_since(self.last_reconnect_attempt) >= Duration::from_secs(2) {
                self.try_connect_vr(false);
            }
            return false;
        }

        if self.vr.as_ref().is_some_and(|v| v.poll_quit_requested()) {
            self.disconnect_vr("SteamVR shutting down; disconnected.");
            return false;
        }

        let frame = self.vr.as_mut().unwrap().snapshot();
        self.latest_frame = frame.clone();

        if !self.auto_sync {
            self.disarm();
            self.phase = DetectorPhase::Paused;
            return false;
        }

        if let Some(until) = self.cooldown_until {
            if now < until {
                let rem = until.duration_since(now).as_secs_f32();
                self.has_prev = false;
                self.reset_still();
                self.baseline = None;
                self.phase = DetectorPhase::Cooldown {
                    remaining_s: rem,
                    last_status: self.last_sync_kind.unwrap_or(SyncResultKind::Synced),
                };
                return false;
            }
            self.cooldown_until = None;
        }

        if !frame.ready_for_solver() {
            let all_connected = frame.has_connected_hmd && frame.connected_tracker_count == 3;
            let within_grace = all_connected
                && (self.baseline.is_some() || self.still_timer > 0.0)
                && now.duration_since(self.last_valid_pose_tick).as_secs_f32() <= POSE_DROP_GRACE_SECONDS;

            if within_grace {
                self.has_prev = false;
                return false;
            }

            let cnt = frame.connected_tracker_count as isize;
            if self.last_device_count_log != cnt {
                let msg = if frame.connected_tracker_count > 3 {
                    format!(
                        "[Waiting] {} Ultimate Trackers connected, but QuestUTSync needs exactly 3 (waist + both feet). Turn the extra trackers off.",
                        frame.connected_tracker_count
                    )
                } else {
                    format!(
                        "[Waiting] Found HMD={}, Ultimate Trackers={}/3...",
                        frame.has_connected_hmd, frame.connected_tracker_count
                    )
                };
                self.log(LogLevel::Info, msg);
                self.last_device_count_log = cnt;
            }
            self.has_prev = false;
            self.reset_still();
            self.baseline = None;
            self.phase = DetectorPhase::WaitingForDevices;
            return false;
        }

        self.last_valid_pose_tick = now;
        if self.last_device_count_log != 3 {
            self.log(
                LogLevel::Success,
                format!(
                    "[Tracked] HMD ({}) + 3 Ultimate Trackers ({}, {}, {}) active.",
                    frame.hmd_serial,
                    frame.solver_serials[0],
                    frame.solver_serials[1],
                    frame.solver_serials[2]
                ),
            );
            self.last_device_count_log = 3;
        }

        let dt = now.duration_since(self.last_tick).as_secs_f32().clamp(0.001, 0.25);
        self.last_tick = now;

        if !self.has_prev {
            self.prev_hmd_pos = frame.hmd_pos;
            self.prev_trk_pos = frame.solver_pos;
            self.has_prev = true;
            self.phase = DetectorPhase::StandStill;
            return false;
        }

        let hmd_speed = if frame.hmd_linear_speed > 0.0 {
            frame.hmd_linear_speed
        } else {
            frame.hmd_pos.distance(self.prev_hmd_pos) / dt
        };
        let mut max_trk_speed = frame.max_tracker_linear_speed;
        if max_trk_speed <= 0.0 {
            for i in 0..3 {
                let s = frame.solver_pos[i].distance(self.prev_trk_pos[i]) / dt;
                if s > max_trk_speed {
                    max_trk_speed = s;
                }
            }
        }
        self.prev_hmd_pos = frame.hmd_pos;
        self.prev_trk_pos = frame.solver_pos;

        if let Some(base) = self.baseline {
            self.armed_timer += dt;
            if self.armed_timer > ARMED_TIMEOUT_SECONDS {
                self.log(
                    LogLevel::Info,
                    "[Timeout] No leg lift detected within 4.5s. Returning to standby.",
                );
                self.baseline = None;
                self.phase = DetectorPhase::StandStill;
                return false;
            }

            let hmd_drift = Vec2::new(frame.hmd_pos.x, frame.hmd_pos.z)
                .distance(Vec2::new(base.hmd_pos.x, base.hmd_pos.z));
            let waist_drop =
                base.tracker_pos[base.indices.waist].y - frame.solver_pos[base.indices.waist].y;
            if hmd_drift > HEAD_MAX_MOVE_DURING_LIFT || waist_drop > WAIST_MAX_DROP_DURING_LIFT {
                self.log(
                    LogLevel::Warn,
                    "[Reset] Body moved/crouched during leg-lift window. Stand still to re-arm.",
                );
                self.baseline = None;
                self.phase = DetectorPhase::StandStill;
                return false;
            }

            let pos_a = frame.solver_pos[base.indices.foot_a];
            let pos_b = frame.solver_pos[base.indices.foot_b];
            let base_a = base.tracker_pos[base.indices.foot_a];
            let base_b = base.tracker_pos[base.indices.foot_b];

            let rise_a = pos_a.y - base_a.y;
            let rise_b = pos_b.y - base_b.y;
            let move_a = pos_a.distance(base_a);
            let move_b = pos_b.distance(base_b);

            let lifted = if rise_a >= LIFT_HEIGHT_THRESHOLD && move_b <= PLANTED_MAX_MOVE {
                Some((base.indices.foot_a, base.indices.foot_b, rise_a))
            } else if rise_b >= LIFT_HEIGHT_THRESHOLD && move_a <= PLANTED_MAX_MOVE {
                Some((base.indices.foot_b, base.indices.foot_a, rise_b))
            } else {
                None
            };

            self.phase = DetectorPhase::Armed {
                remaining_s: (ARMED_TIMEOUT_SECONDS - self.armed_timer).max(0.0),
                max_rise_cm: rise_a.max(rise_b).max(0.0) * 100.0,
            };

            if let Some((lifted_idx, planted_idx, rise_m)) = lifted {
                self.complete_sync(&frame, base, lifted_idx, planted_idx, rise_m);
                return true;
            }
        } else {
            let is_still = hmd_speed < STILL_VELOCITY_THRESHOLD
                && frame.hmd_angular_speed < STILL_ANGULAR_THRESHOLD
                && max_trk_speed < STILL_VELOCITY_THRESHOLD
                && solver::try_classify_standing_trackers(&frame.solver_pos).is_some();

            if is_still {
                if self.still_sample_count == 0 {
                    self.sum_hmd_pos = Vec3::ZERO;
                    self.sum_hmd_forward = Vec3::ZERO;
                    self.sum_trk_pos = [Vec3::ZERO; 3];
                }
                self.still_jitter_frames = 0;
                self.still_timer += dt;
                self.still_sample_count += 1;
                self.sum_hmd_pos += frame.hmd_pos;
                self.sum_hmd_forward += frame.hmd_forward_flat;
                for i in 0..3 {
                    self.sum_trk_pos[i] += frame.solver_pos[i];
                }

                if self.still_timer >= STILL_DURATION_REQUIRED {
                    let inv_n = 1.0 / self.still_sample_count as f32;
                    let avg_fwd = self.sum_hmd_forward * inv_n;
                    let avg_fwd = if avg_fwd.length_squared() > 1e-5 {
                        avg_fwd.normalize()
                    } else {
                        frame.hmd_forward_flat
                    };
                    let mut avg_trk = [Vec3::ZERO; 3];
                    for i in 0..3 {
                        avg_trk[i] = self.sum_trk_pos[i] * inv_n;
                    }
                    let Some(indices) = solver::try_classify_standing_trackers(&avg_trk) else {
                        self.reset_still();
                        self.phase = DetectorPhase::StandStill;
                        return false;
                    };

                    self.baseline = Some(StandingBaseline {
                        hmd_pos: self.sum_hmd_pos * inv_n,
                        hmd_forward_flat: avg_fwd,
                        tracker_pos: avg_trk,
                        indices,
                    });
                    self.armed_timer = 0.0;
                    self.reset_still();
                    self.phase = DetectorPhase::Armed {
                        remaining_s: ARMED_TIMEOUT_SECONDS,
                        max_rise_cm: 0.0,
                    };
                    self.log(
                        LogLevel::Accent,
                        "[Armed] Standing baseline locked! Lift your LEFT leg (>10 cm) now (lifting the right one flips the result 180 deg)...",
                    );
                    self.beep(660, 120, 0, 0);
                } else {
                    self.phase = DetectorPhase::HoldingStill {
                        progress: (self.still_timer / STILL_DURATION_REQUIRED).clamp(0.0, 1.0),
                    };
                }
            } else {
                let mild_jitter = self.still_sample_count > 0 && {
                    self.still_jitter_frames += 1;
                    self.still_jitter_frames <= 2
                        && hmd_speed < STILL_VELOCITY_THRESHOLD * 2.5
                        && frame.hmd_angular_speed < STILL_ANGULAR_THRESHOLD * 2.5
                        && max_trk_speed < STILL_VELOCITY_THRESHOLD * 2.5
                };
                if !mild_jitter {
                    self.reset_still();
                    self.phase = DetectorPhase::StandStill;
                }
            }
        }

        false
    }

    fn complete_sync(
        &mut self,
        frame: &FrameSnapshot,
        base: StandingBaseline,
        lifted_idx: usize,
        planted_idx: usize,
        rise_m: f32,
    ) {
        let waist_idx = base.indices.waist;
        self.log(
            LogLevel::Accent,
            format!(
                "[Leg Lift Detected!] Left Foot = {} (+{:.1} cm), Right Foot = {}, Waist = {}",
                frame.solver_serials[lifted_idx],
                rise_m * 100.0,
                frame.solver_serials[planted_idx],
                frame.solver_serials[waist_idx]
            ),
        );

        self.assigned_roles.clear();
        self.assigned_roles.insert(frame.solver_serials[waist_idx].clone(), "Waist");
        self.assigned_roles.insert(frame.solver_serials[lifted_idx].clone(), "Left Foot");
        self.assigned_roles.insert(frame.solver_serials[planted_idx].clone(), "Right Foot");

        let disk_mtime = pipe::file_mtime(&self.calib_path);
        let current_cal = match (self.last_sent_cal, self.last_cal_file_mtime) {
            (Some(c), m) if m == disk_mtime => c,
            _ => {
                let (c, warn) = pipe::read_current_calibration(&self.calib_path);
                if let Some(w) = warn {
                    self.log(LogLevel::Warn, format!("[Calibration.json] {w}"));
                }
                c
            }
        };

        let delta = solver::solve_delta_transform(
            base.hmd_pos,
            base.hmd_forward_flat,
            base.tracker_pos[waist_idx],
            base.tracker_pos[lifted_idx],
            base.tracker_pos[planted_idx],
            self.feet_back_cm / 100.0,
        );
        let new_cal = solver::compose_calibration(current_cal, delta);
        self.last_delta = Some(delta);
        self.set_active_calibration(new_cal);

        self.log(
            LogLevel::Info,
            format!(
                "[Solver] Delta Yaw = {:.2} deg, Delta Pos = ({:.3}, {:.3}, {:.3})",
                delta.yaw_degrees(),
                delta.translation.x,
                delta.translation.y,
                delta.translation.z
            ),
        );
        self.log(
            LogLevel::Info,
            format!(
                "[Solver] New Global -> Pos=({:.3}, {:.3}, {:.3}), RotXYZW=({:.4}, {:.4}, {:.4}, {:.4})",
                new_cal.position.x,
                new_cal.position.y,
                new_cal.position.z,
                new_cal.rotation.x,
                new_cal.rotation.y,
                new_cal.rotation.z,
                new_cal.rotation.w
            ),
        );

        let kind = if self.dry_run {
            self.log(LogLevel::Info, "[Dry Run] Computed alignment; pipe not written.");
            SyncResultKind::DryRun
        } else {
            if let Some(vr) = self.vr.as_ref() {
                match vr.assign_tracker_roles(
                    &frame.solver_paths[waist_idx],
                    &frame.solver_paths[lifted_idx],
                    &frame.solver_paths[planted_idx],
                ) {
                    Ok(msg) => self.log(LogLevel::Info, format!("[SteamVR Roles] {msg}")),
                    Err(err) => self.log(LogLevel::Warn, format!("[SteamVR Roles Warning] {err}")),
                }
            }

            match pipe::try_send_calibration(PIPE_NAME, new_cal) {
                SendOutcome::Acked(msg) => {
                    self.last_sent_cal = Some(new_cal);
                    self.last_cal_file_mtime = pipe::file_mtime(&self.calib_path);
                    self.sync_count += 1;
                    self.log(LogLevel::Success, format!("[Synced!] {msg}"));
                    self.beep(880, 110, 1320, 160);
                    SyncResultKind::Synced
                }
                SendOutcome::Unconfirmed(msg) => {
                    // Not confirmed applied: never compose the next sync on top of it, re-read Calibration.json instead.
                    self.last_sent_cal = None;
                    self.log(LogLevel::Warn, format!("[Unconfirmed] {msg}"));
                    self.beep(440, 350, 0, 0);
                    SyncResultKind::Unconfirmed
                }
                SendOutcome::Failed(msg) => {
                    self.log(LogLevel::Error, format!("[Pipe Warning] {msg}"));
                    SyncResultKind::Failed
                }
            }
        };

        self.last_sync_kind = Some(kind);
        self.baseline = None;
        self.cooldown_until = Some(Instant::now() + Duration::from_secs_f32(COOLDOWN_SECONDS));
        self.phase = DetectorPhase::Cooldown {
            remaining_s: COOLDOWN_SECONDS,
            last_status: kind,
        };
    }

    fn reset_still(&mut self) {
        self.still_timer = 0.0;
        self.still_sample_count = 0;
        self.still_jitter_frames = 0;
    }

    fn beep(&self, freq1: u32, ms1: u32, freq2: u32, ms2: u32) {
        if !self.audio_beeps {
            return;
        }
        std::thread::spawn(move || unsafe {
            Beep(freq1, ms1);
            if freq2 > 0 && ms2 > 0 {
                Beep(freq2, ms2);
            }
        });
    }
}
