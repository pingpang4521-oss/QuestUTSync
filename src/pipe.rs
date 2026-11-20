use crate::solver::CalibrationTransform;
use glam::{Quat, Vec3};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
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
const CMD_NOTIFY_START_SPACE_CALIBRATION: u32 = 0x1E;
const CMD_SUBMIT_SPACE_CALIBRATION_TRANSFORM: u32 = 0x1F;

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
