//! Shared integration bridge between QuestUTSync (`G:\Ultimate`) and SleeveJoyConVR
//! (`G:\Handtracking + Switch controllers`).
//!
//! - Shares `%LOCALAPPDATA%\SleeveJoyConVR\sleeve_config.json` so Left/Right hand sleeve
//!   tracker assignments and L1/L2/R1/R2 button mappings stay live-synced across both apps.
//! - Excludes assigned Hand Sleeve trackers from QuestUTSync's 3-tracker FBT leg-lift solver
//!   so 5+ VIVE Ultimate Trackers (Waist + L/R Feet + L/R Hands) calibrate together seamlessly.

use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

const SLEEVE_UI_ADDR: &str = "127.0.0.1:21339";

#[derive(Debug, Clone)]
pub struct SleeveBridge {
    config_path: PathBuf,
    last_mtime: Option<SystemTime>,
    last_poll: Instant,
    last_http_poll: Instant,
    pub left_serial: Option<String>,
    pub right_serial: Option<String>,
    pub l1: String,
    pub l2: String,
    pub r1: String,
    pub r2: String,
    pub ui_running: bool,
    pub streaming: bool,
    pub status_note: Option<String>,
}

impl SleeveBridge {
    pub fn new() -> Self {
        let config_path = shared_config_path();
        let mut b = Self {
            config_path,
            last_mtime: None,
            last_poll: Instant::now() - Duration::from_secs(10),
            last_http_poll: Instant::now() - Duration::from_secs(10),
            left_serial: None,
            right_serial: None,
            l1: "grip".to_string(),
            l2: "trigger".to_string(),
            r1: "grip".to_string(),
            r2: "trigger".to_string(),
            ui_running: false,
            streaming: false,
            status_note: None,
        };
        b.reload_from_disk(true);
        b
    }

    pub fn poll(&mut self) {
        if self.last_poll.elapsed() >= Duration::from_millis(700) {
            self.last_poll = Instant::now();
            self.reload_from_disk(false);
        }
        if self.last_http_poll.elapsed() >= Duration::from_secs(2) {
            self.last_http_poll = Instant::now();
            self.probe_http_state();
        }
    }

    pub fn reload_from_disk(&mut self, force: bool) {
        let mt = fs::metadata(&self.config_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if !force && mt == self.last_mtime {
            return;
        }
        self.last_mtime = mt;

        let Ok(text) = fs::read_to_string(&self.config_path) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        self.left_serial = v
            .get("left_tracker_serial")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        self.right_serial = v
            .get("right_tracker_serial")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if let Some(lb) = v.get("left_buttons") {
            if let Some(s) = lb.get("l1").and_then(|x| x.as_str()) {
                self.l1 = s.to_string();
            }
            if let Some(s) = lb.get("l2").and_then(|x| x.as_str()) {
                self.l2 = s.to_string();
            }
        }
        if let Some(rb) = v.get("right_buttons") {
            if let Some(s) = rb.get("r1").and_then(|x| x.as_str()) {
                self.r1 = s.to_string();
            }
            if let Some(s) = rb.get("r2").and_then(|x| x.as_str()) {
                self.r2 = s.to_string();
            }
        }
    }

    pub fn toggle_hand_serial(&mut self, is_left: bool, serial: &str) -> Result<String, String> {
        let mut root = load_or_init_json(&self.config_path);
        let already = if is_left {
            self.left_serial.as_deref() == Some(serial)
        } else {
            self.right_serial.as_deref() == Some(serial)
        };

        let new_val = if already { None } else { Some(serial.to_string()) };
        if is_left {
            if new_val.as_deref() == self.right_serial.as_deref() && new_val.is_some() {
                self.right_serial = None;
                root["right_tracker_serial"] = Value::Null;
            }
            self.left_serial = new_val.clone();
            root["left_tracker_serial"] = match new_val {
                Some(s) => Value::String(s),
                None => Value::Null,
            };
        } else {
            if new_val.as_deref() == self.left_serial.as_deref() && new_val.is_some() {
                self.left_serial = None;
                root["left_tracker_serial"] = Value::Null;
            }
            self.right_serial = new_val.clone();
            root["right_tracker_serial"] = match new_val {
                Some(s) => Value::String(s),
                None => Value::Null,
            };
        }

        save_json(&self.config_path, &root)?;
        self.last_mtime = fs::metadata(&self.config_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let side = if is_left { "Left Hand Sleeve" } else { "Right Hand Sleeve" };
        let msg = if already {
            format!("Cleared {side} assignment for {serial} (returned to FBT pool).")
        } else {
            format!("Assigned {serial} as {side} (excluded from 3-tracker FBT sync).")
        };
        self.status_note = Some(msg.clone());
        Ok(msg)
    }

    pub fn swap_shoulders(&mut self, is_left: bool) -> Result<String, String> {
        let mut root = load_or_init_json(&self.config_path);
        if is_left {
            std::mem::swap(&mut self.l1, &mut self.l2);
            root["left_buttons"]["l1"] = Value::String(self.l1.clone());
            root["left_buttons"]["l2"] = Value::String(self.l2.clone());
        } else {
            std::mem::swap(&mut self.r1, &mut self.r2);
            root["right_buttons"]["r1"] = Value::String(self.r1.clone());
            root["right_buttons"]["r2"] = Value::String(self.r2.clone());
        }
        save_json(&self.config_path, &root)?;
        self.last_mtime = fs::metadata(&self.config_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let msg = if is_left {
            format!("Swapped Left shoulders: L1={}, L2={}", self.l1, self.l2)
        } else {
            format!("Swapped Right shoulders: R1={}, R2={}", self.r1, self.r2)
        };
        self.status_note = Some(msg.clone());
        Ok(msg)
    }

    pub fn probe_http_state(&mut self) {
        match http_request("GET", "/api/state", None) {
            Some(body) => {
                self.ui_running = true;
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    self.streaming = v.get("streaming").and_then(|x| x.as_bool()).unwrap_or(false);
                }
            }
            None => {
                self.ui_running = false;
                self.streaming = false;
            }
        }
    }

    pub fn set_streaming(&mut self, enabled: bool) -> Result<String, String> {
        if enabled && !self.ui_running {
            self.spawn_sleeve_app(false)?;
            std::thread::sleep(Duration::from_millis(350));
        }
        let body = format!(r#"{{"enabled":{enabled}}}"#);
        if http_request("POST", "/api/stream", Some(&body)).is_some() {
            self.ui_running = true;
            self.streaming = enabled;
            let msg = if enabled {
                "Started SleeveJoyConVR 120 Hz Hand Bridge.".to_string()
            } else {
                "Paused SleeveJoyConVR Hand Bridge.".to_string()
            };
            self.status_note = Some(msg.clone());
            Ok(msg)
        } else {
            Err("Could not reach SleeveJoyConVR on 127.0.0.1:21339.".to_string())
        }
    }

    pub fn open_sleeve_ui(&mut self) -> Result<String, String> {
        self.probe_http_state();
        if self.ui_running {
            let _ = Command::new("cmd")
                .args(["/C", "start", "", &format!("http://{SLEEVE_UI_ADDR}")])
                .spawn();
            Ok("Opened SleeveJoyConVR Control Center in browser.".to_string())
        } else {
            self.spawn_sleeve_app(true)?;
            self.ui_running = true;
            Ok("Launched SleeveJoyConVR Control Center.".to_string())
        }
    }

    fn spawn_sleeve_app(&self, open_browser: bool) -> Result<(), String> {
        let exe = find_sleeve_app_exe().ok_or_else(|| {
            r"sleeve-app.exe not found in G:\Handtracking + Switch controllers\target\release\"
                .to_string()
        })?;
        let mut cmd = Command::new(&exe);
        cmd.arg("ui");
        if !open_browser {
            cmd.arg("--no-open");
        }
        // Run from workspace root so vendor/openvr_api.dll and driver_assets/ resolve cleanly.
        if let Some(ws) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            cmd.current_dir(ws);
        }
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| format!("Failed launching {}: {e}", exe.display()))
    }
}

pub fn shared_config_path() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local)
            .join("SleeveJoyConVR")
            .join("sleeve_config.json");
    }
    PathBuf::from(r"G:\Handtracking + Switch controllers\sleeve_config.json")
}

fn find_sleeve_app_exe() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(r"G:\Handtracking + Switch controllers\target\release\sleeve-app.exe"),
        PathBuf::from(r"..\Handtracking + Switch controllers\target\release\sleeve-app.exe"),
        PathBuf::from(r"G:\Handtracking + Switch controllers\target\debug\sleeve-app.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn load_or_init_json(path: &Path) -> Value {
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if v.is_object() {
                return v;
            }
        }
    }
    json!({
        "preset": "vive_ultimate",
        "left_tracker_serial": null,
        "right_tracker_serial": null,
        "left_offset": { "pos_m": [0.005, -0.045, -0.015], "rot_deg": [-15.0, 0.0, 0.0] },
        "right_offset": { "pos_m": [-0.005, -0.045, -0.015], "rot_deg": [-15.0, 0.0, 0.0] },
        "left_buttons": {
            "l1": "grip", "l2": "trigger", "sl": "grip", "sr": "grip",
            "down": "primary", "up": "secondary", "left": "primary_touch", "right": "secondary_touch",
            "minus": "system", "capture": "system", "l3": "stick_click"
        },
        "right_buttons": {
            "r1": "grip", "r2": "trigger", "sl": "grip", "sr": "grip",
            "a": "primary", "b": "secondary", "x": "secondary_touch", "y": "primary_touch",
            "plus": "system", "home": "system", "r3": "stick_click"
        },
        "stick_deadzone": 0.1,
        "rumble_enabled": true,
        "poll_hz": 120,
        "pose_prediction_ms": 8.0
    })
}

fn save_json(path: &Path, val: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pretty = serde_json::to_string_pretty(val).map_err(|e| e.to_string())?;
    fs::write(path, pretty).map_err(|e| format!("Failed saving {}: {e}", path.display()))
}

fn http_request(method: &str, path: &str, body: Option<&str>) -> Option<String> {
    let addr: SocketAddr = SLEEVE_UI_ADDR.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(60)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(120)));

    let req = if let Some(b) = body {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: {SLEEVE_UI_ADDR}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        )
    } else {
        format!("{method} {path} HTTP/1.1\r\nHost: {SLEEVE_UI_ADDR}\r\nConnection: close\r\n\r\n")
    };
    stream.write_all(req.as_bytes()).ok()?;

    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    let (_, resp_body) = raw.split_once("\r\n\r\n")?;
    Some(resp_body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_json_preserves_fields_on_toggle_and_swap() {
        let tmp = std::env::temp_dir().join("questutsync_sleeve_bridge_test.json");
        let _ = fs::remove_file(&tmp);

        let mut b = SleeveBridge {
            config_path: tmp.clone(),
            last_mtime: None,
            last_poll: Instant::now(),
            last_http_poll: Instant::now(),
            left_serial: None,
            right_serial: None,
            l1: "grip".into(),
            l2: "trigger".into(),
            r1: "grip".into(),
            r2: "trigger".into(),
            ui_running: false,
            streaming: false,
            status_note: None,
        };

        b.toggle_hand_serial(true, "48-A0001").unwrap();
        b.toggle_hand_serial(false, "48-A0002").unwrap();
        b.swap_shoulders(true).unwrap();

        let text = fs::read_to_string(&tmp).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["left_tracker_serial"], "48-A0001");
        assert_eq!(v["right_tracker_serial"], "48-A0002");
        assert_eq!(v["left_buttons"]["l1"], "trigger");
        assert_eq!(v["left_buttons"]["l2"], "grip");
        assert_eq!(v["poll_hz"], 120);

        let _ = fs::remove_file(&tmp);
    }
}
