use crate::solver::CalibrationTransform;
use glam::{Quat, Vec3};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, WaitForSingleObject,
};

pub const PIPE_NAME: &str = "TrackerNamedPipe";
const CLIENT_TO_SERVER_MAGIC: u32 = 0x1111_1111;
/// 8-byte `VSWNamedPipeService` query envelope (`[0xEF, 0xFD, 0xDE, 0x00, req_idx_le32]`)
/// required to route commands to `OETServer::OnPipeQueryData` (e.g. pairing/unpairing).
const VSW_QUERY_ENVELOPE_MAGIC: [u8; 4] = [0xEF, 0xFD, 0xDE, 0x00];
pub const CMD_ENUMERATE_TRACKERS: u32 = 0x17;
pub const CMD_PAIR_TRACKER_WITH_DONGLE: u32 = 0x1B;
pub const CMD_UNPAIR_TRACKER_WITH_DONGLE: u32 = 0x1C;
const CMD_NOTIFY_START_SPACE_CALIBRATION: u32 = 0x1E;
const CMD_SUBMIT_SPACE_CALIBRATION_TRANSFORM: u32 = 0x1F;
pub const CMD_START_MAP_CREATION: u32 = 0x21;
pub const CMD_END_MAP_CREATION: u32 = 0x22;
pub const CMD_GET_HOST_HANDLE: u32 = 0x26;
pub const CMD_CANCEL_MAP_CREATION: u32 = 0x27;
pub const CMD_GET_SETUP_STATUS: u32 = 0x28;
pub const CMD_GET_DEVICE_INDICATOR: u32 = 0x2C;
pub const CMD_MAP_SCAN_NOTIFICATION: u32 = 0x30;
pub const CMD_GET_TRACKER_COUNT: u32 = 0x34;

pub const NTFY_PAIR_INFO: u32 = 0x04;
pub const NTFY_SETUP_STATUS: u32 = 0x06;
pub const NTFY_STATUS_INDICATOR: u32 = 0x08;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_SEM_TIMEOUT: u32 = 121;
const ERROR_PIPE_BUSY: u32 = 231;

pub fn default_vive_calib_path() -> PathBuf {
    let program_data = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    program_data
        .join("HTC")
        .join("ViveSoftware")
        .join("ViveUltimateTracker")
        .join("config")
        .join("Calibration.json")
}

pub fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

pub fn read_current_calibration(path: &Path) -> (CalibrationTransform, Option<String>) {
    if !path.exists() {
        return (CalibrationTransform::IDENTITY, None);
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return (CalibrationTransform::IDENTITY, Some(format!("Read warning: {e}"))),
    };
    let root: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (CalibrationTransform::IDENTITY, Some(format!("JSON parse warning: {e}"))),
    };

    if let Some(tf) = root.get("transform") {
        let pos = tf.get("position").and_then(|v| v.as_array());
        let rot = tf.get("rotation").and_then(|v| v.as_array());
        let scale = tf.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;

        if let (Some(p), Some(r)) = (pos, rot) {
            if p.len() == 3 && r.len() == 4 {
                let p_vals: Option<[f32; 3]> = [
                    p[0].as_f64().map(|x| x as f32),
                    p[1].as_f64().map(|x| x as f32),
                    p[2].as_f64().map(|x| x as f32),
                ]
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .and_then(|v| v.try_into().ok());

                let r_vals: Option<[f32; 4]> = [
                    r[0].as_f64().map(|x| x as f32),
                    r[1].as_f64().map(|x| x as f32),
                    r[2].as_f64().map(|x| x as f32),
                    r[3].as_f64().map(|x| x as f32),
                ]
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .and_then(|v| v.try_into().ok());

                if let (Some(pv), Some(rv)) = (p_vals, r_vals) {
                    let q = Quat::from_xyzw(rv[0], rv[1], rv[2], rv[3]);
                    if q.length_squared() > 1e-6 {
                        return (
                            CalibrationTransform {
                                position: Vec3::from_array(pv),
                                rotation: q.normalize(),
                                scale: if scale > 0.0 { scale } else { 1.0 },
                            },
                            None,
                        );
                    }
                }
            }
        }
    }

    (CalibrationTransform::IDENTITY, None)
}

pub fn build_header_packet(cmd: u32, pid: u32, tid: u32, payload_len: u32) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0..4].copy_from_slice(&CLIENT_TO_SERVER_MAGIC.to_le_bytes());
    buf[4..8].copy_from_slice(&cmd.to_le_bytes());
    buf[8..12].copy_from_slice(&pid.to_le_bytes());
    buf[12..16].copy_from_slice(&tid.to_le_bytes());
    buf[16..20].copy_from_slice(&payload_len.to_le_bytes());
    buf
}

pub fn build_submit_transform_packet(cal: CalibrationTransform, pid: u32, tid: u32) -> [u8; 52] {
    let mut buf = [0u8; 52];
    buf[0..20].copy_from_slice(&build_header_packet(
        CMD_SUBMIT_SPACE_CALIBRATION_TRANSFORM,
        pid,
        tid,
        32,
    ));
    buf[20..24].copy_from_slice(&cal.position.x.to_le_bytes());
    buf[24..28].copy_from_slice(&cal.position.y.to_le_bytes());
    buf[28..32].copy_from_slice(&cal.position.z.to_le_bytes());
    buf[32..36].copy_from_slice(&cal.rotation.x.to_le_bytes());
    buf[36..40].copy_from_slice(&cal.rotation.y.to_le_bytes());
    buf[40..44].copy_from_slice(&cal.rotation.z.to_le_bytes());
    buf[44..48].copy_from_slice(&cal.rotation.w.to_le_bytes());
    buf[48..52].copy_from_slice(&cal.scale.to_le_bytes());
    buf
}

#[derive(Debug, Clone)]
pub enum SendOutcome {
    Acked(String),
    Unconfirmed(String),
    Failed(String),
}

pub fn try_send_calibration(pipe_name: &str, cal: CalibrationTransform) -> SendOutcome {
    let pid = std::process::id();
    let tid = unsafe { GetCurrentThreadId() };
    let start_pkt = build_header_packet(CMD_NOTIFY_START_SPACE_CALIBRATION, pid, tid, 0);
    let submit_pkt = build_submit_transform_packet(cal, pid, tid);
    let full_pipe = format!(r"\\.\pipe\{pipe_name}");
    let wide_path: Vec<u16> = OsStr::new(&full_pipe)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut last_err = String::new();
    for attempt in 1..=3 {
        match send_once(&wide_path, &start_pkt, &submit_pkt) {
            Ok(true) => {
                return SendOutcome::Acked(
                    "ViveTrackerServer ACKed 52-byte Submit_Space_Calibration_Transform (0x1F)."
                        .to_string(),
                );
            }
            Ok(false) => {
                return SendOutcome::Unconfirmed(
                    "Sent 0x1F to ViveTrackerServer but got no acknowledgement; the calibration may not have been applied. Check VIVE Hub is running and this program runs as tracker_setup_lite.exe."
                        .to_string(),
                );
            }
            Err(e) => {
                last_err = e;
                if attempt < 3 {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    SendOutcome::Failed(format!(
        r"Failed to send to \\.\pipe\{pipe_name} ({last_err}) — make sure VIVE Hub / ViveTrackerServer.exe is running."
    ))
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

fn send_once(wide_path: &[u16], start_pkt: &[u8], submit_pkt: &[u8]) -> Result<bool, String> {
    let pipe = open_pipe(wide_path, 800)?;
    write_with_timeout(pipe.0, start_pkt, 500)?;
    unsafe {
        FlushFileBuffers(pipe.0);
    }

    let mut ack_buf = [0u8; 256];
    let _ = read_with_timeout(pipe.0, &mut ack_buf, 120);

    write_with_timeout(pipe.0, submit_pkt, 500)?;
    unsafe {
        FlushFileBuffers(pipe.0);
    }

    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let remaining_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
        if remaining_ms == 0 {
            break;
        }
        let n = read_with_timeout(pipe.0, &mut ack_buf, remaining_ms);
        if n == 0 {
            break;
        }
        if n >= 8 {
            let cmd = u32::from_le_bytes(ack_buf[4..8].try_into().unwrap());
            if cmd == CMD_SUBMIT_SPACE_CALIBRATION_TRANSFORM {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn open_pipe(wide_path: &[u16], timeout_ms: u32) -> Result<OwnedHandle, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    loop {
        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(OwnedHandle(handle));
        }
        let err = unsafe { GetLastError() };
        let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
            return Err(format!("Win32 error {err}"));
        };
        // ERROR_PIPE_BUSY = 231
        if err == 231 {
            let wait_ms = rem.as_millis().min(100) as u32;
            unsafe {
                WaitNamedPipeW(wide_path.as_ptr(), wait_ms.max(1));
            }
        } else {
            return Err(format!("Win32 error {err}"));
        }
    }
}

fn write_with_timeout(pipe: HANDLE, data: &[u8], timeout_ms: u32) -> Result<(), String> {
    let ev = OwnedHandle(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) });
    if ev.0.is_null() {
        return Err("CreateEventW failed".to_string());
    }
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.0;

    let mut written: u32 = 0;
    let ok = unsafe { WriteFile(pipe, data.as_ptr(), data.len() as u32, &mut written, &mut ov) };
    if ok != 0 {
        return Ok(());
    }
    let err = unsafe { GetLastError() };
    if err != ERROR_IO_PENDING {
        return Err(format!("WriteFile error {err}"));
    }

    let wait = unsafe { WaitForSingleObject(ev.0, timeout_ms) };
    if wait != WAIT_OBJECT_0 {
        unsafe {
            CancelIoEx(pipe, &ov);
            let _ = GetOverlappedResult(pipe, &ov, &mut written, 1);
        }
        return Err("Pipe write timed out".to_string());
    }
    let res = unsafe { GetOverlappedResult(pipe, &ov, &mut written, 0) };
    if res == 0 {
        return Err(format!("GetOverlappedResult(write) error {}", unsafe { GetLastError() }));
    }
    Ok(())
}

fn read_with_timeout(pipe: HANDLE, buf: &mut [u8], timeout_ms: u32) -> usize {
    if timeout_ms == 0 {
        return 0;
    }
    let ev = OwnedHandle(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) });
    if ev.0.is_null() {
        return 0;
    }
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.0;

    let mut read_bytes: u32 = 0;
    let ok = unsafe {
        ReadFile(
            pipe,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read_bytes,
            &mut ov,
        )
    };
    if ok != 0 {
        return read_bytes as usize;
    }
    if unsafe { GetLastError() } != ERROR_IO_PENDING {
        return 0;
    }

    let wait = unsafe { WaitForSingleObject(ev.0, timeout_ms) };
    if wait != WAIT_OBJECT_0 {
        unsafe {
            CancelIoEx(pipe, &ov);
            let _ = GetOverlappedResult(pipe, &ov, &mut read_bytes, 1);
        }
        return 0;
    }
    if unsafe { GetOverlappedResult(pipe, &ov, &mut read_bytes, 0) } != 0 {
        read_bytes as usize
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Standalone ViveTrackerServer launcher & Room Scan Pipe Protocol
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapState {
    NoTrackerConnected,
    SetupRequired,
    Ready,
    Unknown(i32),
}

impl MapState {
    pub fn from_raw(v: i32) -> Self {
        match v {
            -1 => Self::NoTrackerConnected,
            0 => Self::SetupRequired,
            1 => Self::Ready,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceCalibrationState {
    MapRequired,
    NotCalibrated,
    Ready,
    NotTracking,
    Unknown(i32),
}

impl SpaceCalibrationState {
    pub fn from_raw(v: i32) -> Self {
        match v {
            -1 => Self::MapRequired,
            0 => Self::NotCalibrated,
            1 => Self::Ready,
            2 => Self::NotTracking,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupStatus {
    pub map_state: MapState,
    pub cali_state: SpaceCalibrationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTracker {
    pub serial: String,
    pub handle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MapScanEvent {
    SetCenter = 0,
    LoScanStarted = 1,
    HiScanStarted = 2,
    ScanEnd = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IndicatorEvent {
    MapCoverage = 0,
    EnvironmentLight = 1,
    MovementOverspeed = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusIndicator {
    pub handle: u64,
    pub event_index: u32,
    pub light_state: u8,
    pub map_coverage: [u8; 8],
}

pub fn server_result_name(code: u32) -> &'static str {
    match code {
        0 => "SUCCESS",
        1 => "DEVICE_NOT_FOUND",
        2 => "HID_ERROR",
        3 => "POWER_NOT_ON",
        4 => "DEVICE_WRONG_MODE",
        5 => "OUT_OF_BOUNDS",
        6 => "FAILED_TO_START_PROGRAM",
        7 => "FAILED_TO_GET_DATA",
        20 => "CLIENT_NOT_FOUND",
        21 => "COMMAND_NOT_FOUND",
        22 => "CREATE_EXIST_TRACKER",
        23 => "WRONG_DATA_FORMAT",
        24 => "RESPOND_TIMEOUT",
        25 => "REQUEST_PAIR_COMMAND_FAILED",
        26 => "FAILED_TO_START_CALIBRATE",
        27 => "FAILED_TO_STOP_CALIBRATE",
        28 => "PAIRING_SLOT_IS_FULL",
        29 => "NO_HOST",
        30 => "FAIL_TO_UPDATE_FW",
        32 => "HANDLE_NOT_FOUND",
        _ => "UNKNOWN_ERROR",
    }
}

pub fn find_vive_tracker_server_exe() -> Option<PathBuf> {
    let rel = Path::new("VIVE Hub")
        .join("VIVE Ultimate Tracker")
        .join("ViveUTServer")
        .join("ViveTrackerServer.exe");

    let mut candidates = Vec::with_capacity(4);
    if let Some(pf) = std::env::var_os("ProgramW6432") {
        candidates.push(PathBuf::from(pf).join(&rel));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(pf).join(&rel));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(pf).join(&rel));
    }
    candidates.push(PathBuf::from(r"C:\Program Files").join(&rel));
    candidates.into_iter().find(|p| p.is_file())
}

fn pipe_wide_path(pipe_name: &str) -> Vec<u16> {
    let full = format!(r"\\.\pipe\{pipe_name}");
    OsStr::new(&full)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Returns true if `\\.\pipe\<pipe_name>` currently exists (even if busy).
pub fn is_pipe_available(pipe_name: &str) -> bool {
    let wide = pipe_wide_path(pipe_name);
    let ok = unsafe { WaitNamedPipeW(wide.as_ptr(), 1) };
    if ok != 0 {
        return true;
    }
    let err = unsafe { GetLastError() };
    err == ERROR_SEM_TIMEOUT || err == ERROR_PIPE_BUSY || (err != ERROR_FILE_NOT_FOUND && err != 0)
}

/// Launches `ViveTrackerServer.exe` silently without opening HTC VIVE Hub (`VHConsole.exe`).
/// Because its parent is not `VHConsole.exe`, `ViveTrackerServer.exe` takes its built-in
/// standalone path ("Skip connecting to ViveHub") and serves `\\.\pipe\TrackerNamedPipe`.
pub fn launch_vive_tracker_server_silent() -> Result<PathBuf, String> {
    let exe = find_vive_tracker_server_exe().ok_or_else(|| {
        "ViveTrackerServer.exe not found under Program Files\\VIVE Hub\\VIVE Ultimate Tracker\\ViveUTServer\\."
            .to_string()
    })?;
    let mut cmd = Command::new(&exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    cmd.spawn()
        .map_err(|e| format!("Failed to launch {}: {e}", exe.display()))?;
    Ok(exe)
}

enum ReadChunk {
    Data(usize),
    Timeout,
    Disconnected(u32),
}

fn read_chunk_with_timeout(pipe: HANDLE, buf: &mut [u8], timeout_ms: u32) -> ReadChunk {
    if timeout_ms == 0 {
        return ReadChunk::Timeout;
    }
    let ev = OwnedHandle(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) });
    if ev.0.is_null() {
        return ReadChunk::Timeout;
    }
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.0;

    let mut read_bytes: u32 = 0;
    let ok = unsafe {
        ReadFile(
            pipe,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut read_bytes,
            &mut ov,
        )
    };
    if ok != 0 {
        return if read_bytes > 0 {
            ReadChunk::Data(read_bytes as usize)
        } else {
            ReadChunk::Disconnected(0)
        };
    }
    let err = unsafe { GetLastError() };
    if err != ERROR_IO_PENDING {
        return ReadChunk::Disconnected(err);
    }

    let wait = unsafe { WaitForSingleObject(ev.0, timeout_ms) };
    if wait != WAIT_OBJECT_0 {
        unsafe {
            CancelIoEx(pipe, &ov);
            let _ = GetOverlappedResult(pipe, &ov, &mut read_bytes, 1);
        }
        return ReadChunk::Timeout;
    }
    if unsafe { GetOverlappedResult(pipe, &ov, &mut read_bytes, 0) } != 0 {
        if read_bytes > 0 {
            ReadChunk::Data(read_bytes as usize)
        } else {
            ReadChunk::Disconnected(0)
        }
    } else {
        ReadChunk::Disconnected(unsafe { GetLastError() })
    }
}

pub fn parse_setup_status_payload(payload: &[u8]) -> Result<SetupStatus, String> {
    if payload.len() < 8 {
        return Err(format!("GetSetupStatus payload too short ({} bytes)", payload.len()));
    }
    let map_raw = i32::from_le_bytes(payload[0..4].try_into().unwrap());
    let cali_raw = i32::from_le_bytes(payload[4..8].try_into().unwrap());
    if payload.len() >= 12 {
        let ret = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        if ret != 0 {
            return Err(format!("GetSetupStatus returned {} ({ret})", server_result_name(ret)));
        }
    }
    Ok(SetupStatus {
        map_state: MapState::from_raw(map_raw),
        cali_state: SpaceCalibrationState::from_raw(cali_raw),
    })
}

pub fn parse_tracker_count_payload(payload: &[u8]) -> Result<u32, String> {
    if payload.len() < 8 {
        return Err(format!("GetTrackerCount payload too short ({} bytes)", payload.len()));
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    let ret = u32::from_le_bytes(payload[4..8].try_into().unwrap());
    if ret != 0 {
        return Err(format!("GetTrackerCount returned {} ({ret})", server_result_name(ret)));
    }
    Ok(count)
}

pub fn parse_host_handle_payload(payload: &[u8]) -> Result<HostTracker, String> {
    if payload.len() < 44 {
        return Err(format!("GetHostHandle payload too short ({} bytes)", payload.len()));
    }
    let ret = u32::from_le_bytes(payload[40..44].try_into().unwrap());
    if ret != 0 {
        return Err(format!("GetHostHandle returned {} ({ret})", server_result_name(ret)));
    }
    let nul = payload[0..32].iter().position(|&b| b == 0).unwrap_or(32);
    let serial = String::from_utf8_lossy(&payload[0..nul]).trim().to_string();
    let handle = u64::from_le_bytes(payload[32..40].try_into().unwrap());
    Ok(HostTracker { serial, handle })
}

pub fn parse_ret_only_payload(name: &str, payload: &[u8]) -> Result<(), String> {
    if payload.len() < 4 {
        return Err(format!("{name} payload too short ({} bytes)", payload.len()));
    }
    let ret = u32::from_le_bytes(payload[0..4].try_into().unwrap());
    if ret == 0 {
        Ok(())
    } else {
        Err(format!("{name} failed: {} ({ret})", server_result_name(ret)))
    }
}

pub fn parse_status_indicator_payload(payload: &[u8]) -> Option<StatusIndicator> {
    if payload.len() < 20 {
        return None;
    }
    let handle = u64::from_le_bytes(payload[0..8].try_into().unwrap());
    let event_index = u32::from_le_bytes(payload[8..12].try_into().unwrap());
    let mut map_coverage = [0u8; 8];
    map_coverage.copy_from_slice(&payload[12..20]);
    let light_state = payload[12];
    Some(StatusIndicator {
        handle,
        event_index,
        light_state,
        map_coverage,
    })
}

pub fn parse_device_indicator_return(payload: &[u8]) -> Result<StatusIndicator, String> {
    if payload.len() < 24 {
        return Err(format!(
            "GetDeviceIndicator payload too short ({} bytes)",
            payload.len()
        ));
    }
    let ret = u32::from_le_bytes(payload[20..24].try_into().unwrap());
    if ret != 0 {
        return Err(format!(
            "GetDeviceIndicator returned {} ({ret})",
            server_result_name(ret)
        ));
    }
    parse_status_indicator_payload(&payload[0..20])
        .ok_or_else(|| "Invalid StatusIndicator bytes".to_string())
}

pub fn build_device_indicator_query(handle: u64, event: IndicatorEvent) -> [u8; 12] {
    let mut buf = [0u8; 12];
    buf[0..8].copy_from_slice(&handle.to_le_bytes());
    buf[8..12].copy_from_slice(&(event as u32).to_le_bytes());
    buf
}

pub fn build_map_scan_notification_query(event: MapScanEvent, value: u32) -> [u8; 8] {
    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&(event as u32).to_le_bytes());
    buf[4..8].copy_from_slice(&value.to_le_bytes());
    buf
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumeratedTracker {
    pub serial: String,
    pub openvr_index: u32,
    pub display_num: u32,
}

pub fn parse_enumerate_trackers_payload(payload: &[u8]) -> Vec<EnumeratedTracker> {
    if payload.len() < 8 {
        return Vec::new();
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()).min(10) as usize;
    // EnumerateTrackers_return: 8-byte header (tracker_count + pad/ret) followed by 40-byte items
    // (32-byte serial + u32 openvr_index + u32 display_num). Handle both 4-byte and 8-byte header.
    let item_size = 40;
    let start = if payload.len() >= 8 + count * item_size {
        8
    } else if payload.len() >= 4 + count * item_size {
        4
    } else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = start + i * item_size;
        if base + item_size > payload.len() {
            break;
        }
        let nul = payload[base..base + 32]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(32);
        let serial = String::from_utf8_lossy(&payload[base..base + nul])
            .trim()
            .to_string();
        let openvr_index = u32::from_le_bytes(payload[base + 32..base + 36].try_into().unwrap());
        let display_num = u32::from_le_bytes(payload[base + 36..base + 40].try_into().unwrap());
        if !serial.is_empty() {
            out.push(EnumeratedTracker {
                serial,
                openvr_index,
                display_num,
            });
        }
    }
    out
}

/// Persistent client for `\\.\pipe\TrackerNamedPipe` supporting both query/response
/// and server-pushed notification frames (`0x04`, `0x06`, `0x08`, etc.).
pub struct PipeClient {
    handle: OwnedHandle,
    rx_buf: Vec<u8>,
    next_req_idx: u32,
    disconnected: bool,
}

impl PipeClient {
    pub fn connect(pipe_name: &str, timeout_ms: u32) -> Result<Self, String> {
        let wide = pipe_wide_path(pipe_name);
        let handle = open_pipe(&wide, timeout_ms)?;
        Ok(Self {
            handle,
            rx_buf: Vec::with_capacity(512),
            next_req_idx: 1,
            disconnected: false,
        })
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    fn next_buffered_packet(&mut self) -> Option<(u32, Vec<u8>)> {
        // Strip VSWNamedPipeService 8-byte query-response envelope (`0xEF 0xFD .. .. <req_idx>`).
        if self.rx_buf.len() >= 28 && self.rx_buf[0] == 0xEF && self.rx_buf[1] == 0xFD {
            let inner_len = u32::from_le_bytes(self.rx_buf[24..28].try_into().unwrap()) as usize;
            if inner_len > 65_536 {
                self.rx_buf.clear();
                self.disconnected = true;
                return None;
            }
            let total = 8 + 20 + inner_len;
            if self.rx_buf.len() < total {
                return None;
            }
            let cmd = u32::from_le_bytes(self.rx_buf[12..16].try_into().unwrap());
            let payload = self.rx_buf[28..total].to_vec();
            self.rx_buf.drain(0..total);
            return Some((cmd, payload));
        }
        if self.rx_buf.len() < 20 {
            return None;
        }
        let cmd = u32::from_le_bytes(self.rx_buf[4..8].try_into().unwrap());
        let data_len = u32::from_le_bytes(self.rx_buf[16..20].try_into().unwrap()) as usize;
        if data_len > 65_536 {
            // Desynchronized stream — drop buffer and force reconnect on next error.
            self.rx_buf.clear();
            self.disconnected = true;
            return None;
        }
        let total = 20 + data_len;
        if self.rx_buf.len() < total {
            return None;
        }
        let payload = self.rx_buf[20..total].to_vec();
        self.rx_buf.drain(0..total);
        Some((cmd, payload))
    }

    pub fn send_query(
        &mut self,
        cmd: u32,
        payload: &[u8],
        timeout_ms: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<Vec<u8>, String> {
        self.send_framed(false, cmd, payload, timeout_ms, on_ntfy)
    }

    /// Sends a command wrapped in the 8-byte `VSWNamedPipeService` query envelope so
    /// `ViveTrackerServer.exe` routes it to `OETServer::OnPipeQueryData` (required for pairing).
    pub fn send_vsw_query(
        &mut self,
        cmd: u32,
        payload: &[u8],
        timeout_ms: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<Vec<u8>, String> {
        self.send_framed(true, cmd, payload, timeout_ms, on_ntfy)
    }

    fn send_framed(
        &mut self,
        use_vsw_envelope: bool,
        cmd: u32,
        payload: &[u8],
        timeout_ms: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<Vec<u8>, String> {
        if self.disconnected {
            return Err("Pipe disconnected".to_string());
        }
        let pid = std::process::id();
        let tid = unsafe { GetCurrentThreadId() };
        let hdr = build_header_packet(cmd, pid, tid, payload.len() as u32);
        let mut frame = Vec::with_capacity(28 + payload.len());
        if use_vsw_envelope {
            let req_idx = self.next_req_idx;
            self.next_req_idx = self.next_req_idx.wrapping_add(1).max(1);
            frame.extend_from_slice(&VSW_QUERY_ENVELOPE_MAGIC);
            frame.extend_from_slice(&req_idx.to_le_bytes());
        }
        frame.extend_from_slice(&hdr);
        frame.extend_from_slice(payload);

        if let Err(e) = write_with_timeout(self.handle.0, &frame, 500) {
            self.disconnected = true;
            return Err(e);
        }
        unsafe {
            FlushFileBuffers(self.handle.0);
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut tmp = [0u8; 1024];

        loop {
            while let Some((rx_cmd, rx_payload)) = self.next_buffered_packet() {
                if rx_cmd == cmd {
                    return Ok(rx_payload);
                }
                on_ntfy(rx_cmd, &rx_payload);
            }
            if self.disconnected {
                return Err("Pipe stream desynced".to_string());
            }
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                return Err(format!("Timed out waiting for response to cmd 0x{cmd:02X}"));
            };
            let rem_ms = rem.as_millis().min(u32::MAX as u128) as u32;
            if rem_ms == 0 {
                return Err(format!("Timed out waiting for response to cmd 0x{cmd:02X}"));
            }
            match read_chunk_with_timeout(self.handle.0, &mut tmp, rem_ms) {
                ReadChunk::Data(n) => self.rx_buf.extend_from_slice(&tmp[..n]),
                ReadChunk::Timeout => {
                    return Err(format!("Timed out waiting for response to cmd 0x{cmd:02X}"));
                }
                ReadChunk::Disconnected(err) => {
                    self.disconnected = true;
                    return Err(format!("Pipe closed (Win32 {err})"));
                }
            }
        }
    }

    pub fn drain_notifications(
        &mut self,
        timeout_ms: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) {
        if self.disconnected {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut tmp = [0u8; 1024];
        loop {
            while let Some((rx_cmd, rx_payload)) = self.next_buffered_packet() {
                on_ntfy(rx_cmd, &rx_payload);
            }
            if self.disconnected {
                break;
            }
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let rem_ms = rem.as_millis().min(u32::MAX as u128) as u32;
            if rem_ms == 0 {
                break;
            }
            match read_chunk_with_timeout(self.handle.0, &mut tmp, rem_ms) {
                ReadChunk::Data(n) => self.rx_buf.extend_from_slice(&tmp[..n]),
                ReadChunk::Timeout => break,
                ReadChunk::Disconnected(_) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
    }

    pub fn get_setup_status(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<SetupStatus, String> {
        let p = self.send_query(CMD_GET_SETUP_STATUS, &[], 500, on_ntfy)?;
        parse_setup_status_payload(&p)
    }

    pub fn get_tracker_count(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<u32, String> {
        let p = self.send_query(CMD_GET_TRACKER_COUNT, &[], 500, on_ntfy)?;
        parse_tracker_count_payload(&p)
    }

    pub fn get_host_handle(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<HostTracker, String> {
        let p = self.send_query(CMD_GET_HOST_HANDLE, &[], 500, on_ntfy)?;
        parse_host_handle_payload(&p)
    }

    pub fn start_map_creation(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let p = self.send_query(CMD_START_MAP_CREATION, &[], 3000, on_ntfy)?;
        parse_ret_only_payload("Start_Map_Creation", &p)
    }

    pub fn end_map_creation(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let p = self.send_query(CMD_END_MAP_CREATION, &[], 5000, on_ntfy)?;
        parse_ret_only_payload("End_Map_Creation", &p)
    }

    pub fn cancel_map_creation(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let p = self.send_query(CMD_CANCEL_MAP_CREATION, &[], 2000, on_ntfy)?;
        parse_ret_only_payload("Cancel_Map_Creation", &p)
    }

    pub fn send_map_scan_event(
        &mut self,
        event: MapScanEvent,
        value: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let q = build_map_scan_notification_query(event, value);
        let p = self.send_query(CMD_MAP_SCAN_NOTIFICATION, &q, 1000, on_ntfy)?;
        parse_ret_only_payload("Map_Scan_Notification", &p)
    }

    pub fn get_device_indicator(
        &mut self,
        handle: u64,
        event: IndicatorEvent,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<StatusIndicator, String> {
        let q = build_device_indicator_query(handle, event);
        let p = self.send_query(CMD_GET_DEVICE_INDICATOR, &q, 500, on_ntfy)?;
        parse_device_indicator_return(&p)
    }

    pub fn enumerate_trackers(
        &mut self,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<Vec<EnumeratedTracker>, String> {
        let p = self.send_query(CMD_ENUMERATE_TRACKERS, &[], 500, on_ntfy)?;
        Ok(parse_enumerate_trackers_payload(&p))
    }

    /// Triggers dongle pairing mode on slot `0..=4` via `OnPipeQueryData` (cmd `0x1B`).
    pub fn pair_tracker_with_dongle(
        &mut self,
        slot: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let slot_bytes = slot.min(4).to_le_bytes();
        let p = self.send_vsw_query(CMD_PAIR_TRACKER_WITH_DONGLE, &slot_bytes, 4000, on_ntfy)?;
        parse_ret_only_payload("Pair_Tracker_With_Dongle", &p)
    }

    /// Unpairs dongle slot `0..=4` via `OnPipeQueryData` (cmd `0x1C`).
    pub fn unpair_tracker_with_dongle(
        &mut self,
        slot: u32,
        on_ntfy: &mut impl FnMut(u32, &[u8]),
    ) -> Result<(), String> {
        let slot_bytes = slot.min(4).to_le_bytes();
        let p = self.send_vsw_query(CMD_UNPAIR_TRACKER_WITH_DONGLE, &slot_bytes, 4000, on_ntfy)?;
        parse_ret_only_payload("Unpair_Tracker_With_Dongle", &p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_status_and_host_handle_parsing() {
        let mut setup = Vec::new();
        setup.extend_from_slice(&0i32.to_le_bytes()); // SetupRequired
        setup.extend_from_slice(&(-1i32).to_le_bytes()); // MapRequired
        setup.extend_from_slice(&0u32.to_le_bytes()); // SUCCESS
        let st = parse_setup_status_payload(&setup).unwrap();
        assert_eq!(st.map_state, MapState::SetupRequired);
        assert_eq!(st.cali_state, SpaceCalibrationState::MapRequired);

        let mut host = [0u8; 44];
        host[0..8].copy_from_slice(b"48-A1234");
        host[32..40].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_le_bytes());
        host[40..44].copy_from_slice(&0u32.to_le_bytes());
        let h = parse_host_handle_payload(&host).unwrap();
        assert_eq!(h.serial, "48-A1234");
        assert_eq!(h.handle, 0x1234_5678_9ABC_DEF0);
    }

    #[test]
    fn device_indicator_query_and_response_layout() {
        let q = build_device_indicator_query(42, IndicatorEvent::MapCoverage);
        assert_eq!(q.len(), 12);
        assert_eq!(u64::from_le_bytes(q[0..8].try_into().unwrap()), 42);
        assert_eq!(u32::from_le_bytes(q[8..12].try_into().unwrap()), 0);

        let mut resp = [0u8; 24];
        resp[0..8].copy_from_slice(&42u64.to_le_bytes());
        resp[8..12].copy_from_slice(&0u32.to_le_bytes());
        resp[12..20].copy_from_slice(&[80, 75, 70, 65, 90, 85, 60, 50]);
        resp[20..24].copy_from_slice(&0u32.to_le_bytes());
        let ind = parse_device_indicator_return(&resp).unwrap();
        assert_eq!(ind.handle, 42);
        assert_eq!(ind.event_index, 0);
        assert_eq!(ind.map_coverage, [80, 75, 70, 65, 90, 85, 60, 50]);
    }

    #[test]
    fn enumerate_trackers_and_vsw_query_envelope() {
        assert_eq!(VSW_QUERY_ENVELOPE_MAGIC, [0xEF, 0xFD, 0xDE, 0x00]);
        let hdr = build_header_packet(CMD_PAIR_TRACKER_WITH_DONGLE, 123, 456, 4);
        assert_eq!(u32::from_le_bytes(hdr[0..4].try_into().unwrap()), 0x1111_1111);
        assert_eq!(u32::from_le_bytes(hdr[4..8].try_into().unwrap()), 0x1B);
        assert_eq!(u32::from_le_bytes(hdr[16..20].try_into().unwrap()), 4);

        let mut payload = vec![0u8; 8 + 40];
        payload[0..4].copy_from_slice(&1u32.to_le_bytes());
        payload[8..16].copy_from_slice(b"48-B9999");
        payload[8 + 32..8 + 36].copy_from_slice(&3u32.to_le_bytes());
        payload[8 + 36..8 + 40].copy_from_slice(&2u32.to_le_bytes());
        let list = parse_enumerate_trackers_payload(&payload);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].serial, "48-B9999");
        assert_eq!(list[0].openvr_index, 3);
        assert_eq!(list[0].display_num, 2);
    }
}
