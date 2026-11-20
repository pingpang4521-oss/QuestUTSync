use glam::Vec3;
use std::ffi::{c_char, c_void, CString, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const MAX_TRACKED_DEVICES: usize = 64;
const TRACKING_UNIVERSE_STANDING: i32 = 1;
const TRACKED_DEVICE_CLASS_HMD: i32 = 1;
const TRACKED_DEVICE_CLASS_GENERIC_TRACKER: i32 = 3;

const PROP_MODEL_NUMBER_STRING: i32 = 1001;
const PROP_SERIAL_NUMBER_STRING: i32 = 1002;
const PROP_REGISTERED_DEVICE_TYPE_STRING: i32 = 1036;
const PROP_CONTROLLER_TYPE_STRING: i32 = 7000;
const TRACKED_PROP_BUFFER_TOO_SMALL: i32 = 5;

pub const VR_APP_OVERLAY: i32 = 2;
pub const VR_APP_BACKGROUND: i32 = 3;
pub const VR_INIT_ERR_NO_SERVER_FOR_BACKGROUND: i32 = 121;

pub const VR_EVENT_MOUSE_MOVE: u32 = 300;
pub const VR_EVENT_MOUSE_BUTTON_DOWN: u32 = 301;
pub const VR_EVENT_MOUSE_BUTTON_UP: u32 = 302;
pub const VR_EVENT_SCROLL_DISCRETE: u32 = 305;
pub const VR_EVENT_SCROLL_SMOOTH: u32 = 309;
pub const VR_EVENT_QUIT: u32 = 700;

pub const VR_MOUSE_BUTTON_LEFT: u32 = 1;
pub const VR_MOUSE_BUTTON_RIGHT: u32 = 2;
pub const VR_MOUSE_BUTTON_MIDDLE: u32 = 4;

pub const VR_OVERLAY_INPUT_MOUSE: i32 = 1;
pub const VR_OVERLAY_FLAG_SEND_DISCRETE_SCROLL: u32 = 64;
pub const VR_OVERLAY_FLAG_SEND_SMOOTH_SCROLL: u32 = 131_072;

pub const TEXTURE_TYPE_OPENGL: i32 = 1;
pub const COLOR_SPACE_GAMMA: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HmdMatrix34 {
    m: [f32; 12],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HmdVector3 {
    v: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HmdVector2 {
    pub v: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TrackedDevicePose {
    device_to_absolute: HmdMatrix34,
    velocity: HmdVector3,
    angular_velocity: HmdVector3,
    tracking_result: i32,
    pose_is_valid: bool,
    device_is_connected: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VrTexture {
    pub handle: *mut c_void,
    pub texture_type: i32,
    pub color_space: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VrTextureBounds {
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VrEventMouse {
    pub x: f32,
    pub y: f32,
    pub button: u32,
    pub cursor_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VrEventScroll {
    pub xdelta: f32,
    pub ydelta: f32,
    pub unused: u32,
    pub viewport_scale: f32,
    pub cursor_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union VrEventData {
    pub reserved: [u64; 6],
    pub mouse: VrEventMouse,
    pub scroll: VrEventScroll,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VrEvent {
    pub event_type: u32,
    pub tracked_device_index: u32,
    pub event_age_seconds: f32,
    pub data: VrEventData,
}

impl Default for VrEvent {
    fn default() -> Self {
        Self {
            event_type: 0,
            tracked_device_index: 0,
            event_age_seconds: 0.0,
            data: VrEventData { reserved: [0; 6] },
        }
    }
}

pub fn verify_abi_layout() -> Result<(), String> {
    let event_size = std::mem::size_of::<VrEvent>();
    let data_offset = std::mem::offset_of!(VrEvent, data);
    let pose_size = std::mem::size_of::<TrackedDevicePose>();
    // On Windows x64 (default pack 8): VREvent_t = 12 bytes header + 4 pad + 48 bytes union = 64 bytes.
    if event_size != 64 || data_offset != 16 {
        return Err(format!(
            "VrEvent layout mismatch: size={event_size} (expected 64), data_offset={data_offset} (expected 16)"
        ));
    }
    // TrackedDevicePose_t = 48 + 12 + 12 + 4 + 1 + 1 + 2 pad = 80 bytes.
    if pose_size != 80 {
        return Err(format!("TrackedDevicePose size mismatch: {pose_size} (expected 80)"));
    }
    Ok(())
}

type FnVrInitInternal2 = unsafe extern "C" fn(*mut i32, i32, *const c_char) -> u32;
type FnVrShutdownInternal = unsafe extern "C" fn();
type FnVrGetGenericInterface = unsafe extern "C" fn(*const c_char, *mut i32) -> *const usize;

type FnGetPoses = unsafe extern "system" fn(i32, f32, *mut TrackedDevicePose, u32);
type FnGetDeviceClass = unsafe extern "system" fn(u32) -> i32;
type FnGetStringProp = unsafe extern "system" fn(u32, i32, *mut u8, u32, *mut i32) -> u32;
type FnPollNextEvent = unsafe extern "system" fn(*mut VrEvent, u32) -> bool;
type FnSetSettingsString = unsafe extern "system" fn(*const c_char, *const c_char, *const c_char, *mut i32);

type FnDestroyOverlay = unsafe extern "system" fn(u64) -> i32;
type FnSetOverlayFlag = unsafe extern "system" fn(u64, u32, bool) -> i32;
type FnSetOverlayWidth = unsafe extern "system" fn(u64, f32) -> i32;
type FnSetOverlayTextureBounds = unsafe extern "system" fn(u64, *const VrTextureBounds) -> i32;
type FnIsOverlayVisible = unsafe extern "system" fn(u64) -> bool;
type FnPollNextOverlayEvent = unsafe extern "system" fn(u64, *mut VrEvent, u32) -> bool;
type FnSetOverlayInputMethod = unsafe extern "system" fn(u64, i32) -> i32;
type FnSetOverlayMouseScale = unsafe extern "system" fn(u64, *const HmdVector2) -> i32;
type FnSetOverlayTexture = unsafe extern "system" fn(u64, *const VrTexture) -> i32;
type FnSetOverlayRaw = unsafe extern "system" fn(u64, *const c_void, u32, u32, u32) -> i32;
type FnCreateDashboardOverlay =
    unsafe extern "system" fn(*const c_char, *const c_char, *mut u64, *mut u64) -> i32;
type FnShowDashboard = unsafe extern "system" fn(*const c_char);

#[derive(Clone, Copy)]
pub struct OverlayApi {
    destroy_overlay: FnDestroyOverlay,
    set_overlay_flag: FnSetOverlayFlag,
    set_overlay_width: FnSetOverlayWidth,
    set_overlay_texture_bounds: FnSetOverlayTextureBounds,
    is_overlay_visible: FnIsOverlayVisible,
    poll_next_overlay_event: FnPollNextOverlayEvent,
    set_overlay_input_method: FnSetOverlayInputMethod,
    set_overlay_mouse_scale: FnSetOverlayMouseScale,
    set_overlay_texture: FnSetOverlayTexture,
    set_overlay_raw: FnSetOverlayRaw,
    create_dashboard_overlay: FnCreateDashboardOverlay,
    show_dashboard: FnShowDashboard,
}

impl OverlayApi {
    pub fn create_dashboard_overlay(&self, key: &str, name: &str) -> Result<(u64, u64), i32> {
        let c_key = CString::new(key).map_err(|_| -1)?;
        let c_name = CString::new(name).map_err(|_| -1)?;
        let mut main_handle = 0u64;
        let mut thumb_handle = 0u64;
        let err = unsafe {
            (self.create_dashboard_overlay)(
                c_key.as_ptr(),
                c_name.as_ptr(),
                &mut main_handle,
                &mut thumb_handle,
            )
        };
        if err == 0 {
            Ok((main_handle, thumb_handle))
        } else {
            Err(err)
        }
    }

    pub fn destroy_overlay(&self, handle: u64) {
        if handle != 0 {
            unsafe {
                (self.destroy_overlay)(handle);
            }
        }
    }

    pub fn set_width_in_meters(&self, handle: u64, width: f32) -> i32 {
        unsafe { (self.set_overlay_width)(handle, width) }
    }

    pub fn set_input_method(&self, handle: u64, method: i32) -> i32 {
        unsafe { (self.set_overlay_input_method)(handle, method) }
    }

    pub fn set_flag(&self, handle: u64, flag: u32, enabled: bool) -> i32 {
        unsafe { (self.set_overlay_flag)(handle, flag, enabled) }
    }

    pub fn set_mouse_scale(&self, handle: u64, width: f32, height: f32) -> i32 {
        let scale = HmdVector2 { v: [width, height] };
        unsafe { (self.set_overlay_mouse_scale)(handle, &scale) }
    }

    pub fn set_texture_bounds(&self, handle: u64, bounds: VrTextureBounds) -> i32 {
        unsafe { (self.set_overlay_texture_bounds)(handle, &bounds) }
    }

    pub fn set_texture(&self, handle: u64, tex: VrTexture) -> i32 {
        unsafe { (self.set_overlay_texture)(handle, &tex) }
    }

    pub fn set_raw_rgba(&self, handle: u64, rgba: &[u8], width: u32, height: u32) -> i32 {
        unsafe { (self.set_overlay_raw)(handle, rgba.as_ptr().cast(), width, height, 4) }
    }

    pub fn is_visible(&self, handle: u64) -> bool {
        handle != 0 && unsafe { (self.is_overlay_visible)(handle) }
    }

    pub fn poll_event(&self, handle: u64) -> Option<VrEvent> {
        let mut ev = VrEvent::default();
        let ok = unsafe {
            (self.poll_next_overlay_event)(handle, &mut ev, std::mem::size_of::<VrEvent>() as u32)
        };
        ok.then_some(ev)
    }

    pub fn show_dashboard(&self, key: &str) {
        if let Ok(c_key) = CString::new(key) {
            unsafe {
                (self.show_dashboard)(c_key.as_ptr());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackerReading {
    pub serial: String,
    pub pos: Vec3,
    pub linear_speed: f32,
    pub pose_valid: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FrameSnapshot {
    pub has_connected_hmd: bool,
    pub has_hmd: bool,
    pub hmd_serial: String,
    pub hmd_pos: Vec3,
    pub hmd_forward_flat: Vec3,
    pub hmd_linear_speed: f32,
    pub hmd_angular_speed: f32,
    pub connected_tracker_count: usize,
    pub valid_tracker_count: usize,
    pub max_tracker_linear_speed: f32,
    pub trackers: Vec<TrackerReading>,
    /// Populated when exactly 3 Ultimate Trackers are connected & valid.
    pub solver_pos: [Vec3; 3],
    pub solver_serials: [String; 3],
    pub solver_paths: [String; 3],
}

impl FrameSnapshot {
    pub fn ready_for_solver(&self) -> bool {
        self.has_hmd && self.connected_tracker_count == 3 && self.valid_tracker_count == 3
    }
}

#[derive(Clone, Default)]
struct CachedDevice {
    evaluated: bool,
    is_hmd: bool,
    is_ultimate_tracker: bool,
    serial: String,
    device_path: String,
}

pub struct OpenVrSession {
    module: HMODULE,
    vr_shutdown: FnVrShutdownInternal,
    get_poses: FnGetPoses,
    get_device_class: FnGetDeviceClass,
    get_string_prop: FnGetStringProp,
    poll_next_event: FnPollNextEvent,
    set_settings_string: Option<FnSetSettingsString>,
    overlay: Option<OverlayApi>,
    pose_buffer: [TrackedDevicePose; MAX_TRACKED_DEVICES],
    device_cache: Vec<CachedDevice>,
}

impl OpenVrSession {
    /// Connects to SteamVR. When `want_overlay` is true and `launch_if_closed` is false,
    /// we probe with `VRApplication_Background` first so we never force-launch SteamVR
    /// just by opening the desktop GUI.
    pub fn try_init(want_overlay: bool, launch_if_closed: bool) -> Result<Self, String> {
        let module = load_openvr_dll()?;
        let vr_init: FnVrInitInternal2 = load_symbol(module, b"VR_InitInternal2\0")?;
        let vr_shutdown: FnVrShutdownInternal = load_symbol(module, b"VR_ShutdownInternal\0")?;
        let vr_get_iface: FnVrGetGenericInterface =
            load_symbol(module, b"VR_GetGenericInterface\0")?;

        let app_type = if want_overlay && !launch_if_closed {
            // Probe whether vrserver is already running without force-launching SteamVR.
            let mut probe_err = 0i32;
            unsafe {
                vr_init(&mut probe_err, VR_APP_BACKGROUND, std::ptr::null());
            }
            if probe_err != 0 {
                unsafe {
                    FreeLibrary(module);
                }
                return Err(if probe_err == VR_INIT_ERR_NO_SERVER_FOR_BACKGROUND {
                    "SteamVR is not running (EVRInitError=121).".to_string()
                } else {
                    format!("VR_InitInternal2 returned EVRInitError={probe_err}")
                });
            }
            unsafe {
                vr_shutdown();
            }
            VR_APP_OVERLAY
        } else if want_overlay {
            VR_APP_OVERLAY
        } else {
            VR_APP_BACKGROUND
        };

        let mut err = 0i32;
        unsafe {
            vr_init(&mut err, app_type, std::ptr::null());
        }
        // If overlay mode fails (e.g., headless/non-dashboard runtime), fall back to background mode.
        let mut active_overlay_mode = app_type == VR_APP_OVERLAY;
        if err != 0 && active_overlay_mode {
            err = 0;
            active_overlay_mode = false;
            unsafe {
                vr_init(&mut err, VR_APP_BACKGROUND, std::ptr::null());
            }
        }
        if err != 0 {
            unsafe {
                FreeLibrary(module);
            }
            return Err(format!("VR_InitInternal2 returned EVRInitError={err}"));
        }

        // Only IVRSystem_022: the FnTable slots below (11, 19, 27, 29) are verified for this version only.
        let sys_table = unsafe {
            let mut e = 0i32;
            let p = vr_get_iface(b"FnTable:IVRSystem_022\0".as_ptr().cast(), &mut e);
            if p.is_null() || e != 0 {
                vr_shutdown();
                FreeLibrary(module);
                return Err(format!("SteamVR does not provide IVRSystem_022 (err={e}). Update SteamVR."));
            }
            p
        };

        let get_poses: FnGetPoses = read_slot(sys_table, 11)?;
        let get_device_class: FnGetDeviceClass = read_slot(sys_table, 19)?;
        let get_string_prop: FnGetStringProp = read_slot(sys_table, 27)?;
        let poll_next_event: FnPollNextEvent = read_slot(sys_table, 29)?;

        let set_settings_string: Option<FnSetSettingsString> = unsafe {
            let mut e = 0i32;
            let p = vr_get_iface(b"FnTable:IVRSettings_003\0".as_ptr().cast(), &mut e);
            if !p.is_null() && e == 0 {
                read_slot(p, 4).ok()
            } else {
                None
            }
        };

        let overlay = if active_overlay_mode {
            unsafe {
                let mut e = 0i32;
                let p = vr_get_iface(b"FnTable:IVROverlay_027\0".as_ptr().cast(), &mut e);
                if !p.is_null() && e == 0 {
                    parse_overlay_table(p).ok()
                } else {
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            module,
            vr_shutdown,
            get_poses,
            get_device_class,
            get_string_prop,
            poll_next_event,
            set_settings_string,
            overlay,
            pose_buffer: [TrackedDevicePose::default(); MAX_TRACKED_DEVICES],
            device_cache: vec![CachedDevice::default(); MAX_TRACKED_DEVICES],
        })
    }

    pub fn overlay_api(&self) -> Option<OverlayApi> {
        self.overlay
    }

    /// Drains system events and returns `true` if SteamVR requested quit.
    pub fn poll_quit_requested(&self) -> bool {
        let mut quit = false;
        let mut ev = VrEvent::default();
        while unsafe {
            (self.poll_next_event)(&mut ev, std::mem::size_of::<VrEvent>() as u32)
        } {
            if ev.event_type == VR_EVENT_QUIT {
                quit = true;
            }
        }
        quit
    }

    pub fn assign_tracker_roles(
        &self,
        waist_path: &str,
        left_foot_path: &str,
        right_foot_path: &str,
    ) -> Result<String, String> {
        let Some(set_str) = self.set_settings_string else {
            return Err("IVRSettings_003 unavailable; skipping role assignment.".to_string());
        };
        let mut errs = Vec::new();
        for (path, role) in [
            (waist_path, "TrackerRole_Waist"),
            (left_foot_path, "TrackerRole_LeftFoot"),
            (right_foot_path, "TrackerRole_RightFoot"),
        ] {
            if path.is_empty() {
                continue;
            }
            let Ok(c_sec) = CString::new("trackers") else { continue };
            let Ok(c_key) = CString::new(path) else { continue };
            let Ok(c_val) = CString::new(role) else { continue };
            let mut err = 0i32;
            unsafe {
                set_str(c_sec.as_ptr(), c_key.as_ptr(), c_val.as_ptr(), &mut err);
            }
            if err != 0 {
                errs.push(format!("{path} -> {role} (err={err})"));
            }
        }
        if errs.is_empty() {
            Ok(format!(
                "Assigned Waist={waist_path}, LeftFoot={left_foot_path}, RightFoot={right_foot_path}"
            ))
        } else {
            Err(format!("Failed setting roles: {}", errs.join(", ")))
        }
    }

    pub fn snapshot(&mut self) -> FrameSnapshot {
        let mut snap = FrameSnapshot::default();
        unsafe {
            (self.get_poses)(
                TRACKING_UNIVERSE_STANDING,
                0.0,
                self.pose_buffer.as_mut_ptr(),
                MAX_TRACKED_DEVICES as u32,
            );
        }

        for i in 0..MAX_TRACKED_DEVICES {
            let pose = self.pose_buffer[i];
            if !pose.device_is_connected {
                self.device_cache[i].evaluated = false;
                continue;
            }

            if !self.device_cache[i].evaluated {
                let idx = i as u32;
                let dev_class = unsafe { (self.get_device_class)(idx) };
                let serial = self.get_string_property(idx, PROP_SERIAL_NUMBER_STRING);
                let is_hmd = dev_class == TRACKED_DEVICE_CLASS_HMD;
                let mut is_ut = false;
                let mut dev_path = String::new();

                if dev_class == TRACKED_DEVICE_CLASS_GENERIC_TRACKER {
                    let ctrl_type = self.get_string_property(idx, PROP_CONTROLLER_TYPE_STRING);
                    let reg_path = self.get_string_property(idx, PROP_REGISTERED_DEVICE_TYPE_STRING);
                    let serial_up = serial.to_ascii_uppercase();
                    is_ut = serial_up.starts_with("48-A")
                        || serial_up.starts_with("43A")
                        || ctrl_type.to_ascii_lowercase().contains("ultimate")
                        || reg_path.to_ascii_lowercase().contains("ultimate")
                        || self
                            .get_string_property(idx, PROP_MODEL_NUMBER_STRING)
                            .to_ascii_lowercase()
                            .contains("ultimate");

                    if is_ut {
                        dev_path = if !reg_path.is_empty() {
                            if reg_path.starts_with('/') {
                                reg_path
                            } else {
                                format!("/devices/{reg_path}")
                            }
                        } else {
                            format!("/devices/vive_ultimate_tracker/{serial}")
                        };
                    }
                }

                self.device_cache[i] = CachedDevice {
                    evaluated: true,
                    is_hmd,
                    is_ultimate_tracker: is_ut,
                    serial,
                    device_path: dev_path,
                };
            }

            let cached = &self.device_cache[i];
            if cached.is_hmd {
                snap.has_connected_hmd = true;
                if pose.pose_is_valid && !snap.has_hmd {
                    let m = pose.device_to_absolute.m;
                    snap.has_hmd = true;
                    snap.hmd_serial = cached.serial.clone();
                    snap.hmd_pos = Vec3::new(m[3], m[7], m[11]);
                    snap.hmd_linear_speed = Vec3::from_array(pose.velocity.v).length();
                    snap.hmd_angular_speed = Vec3::from_array(pose.angular_velocity.v).length();
                    let fwd = Vec3::new(-m[2], 0.0, -m[10]);
                    snap.hmd_forward_flat = if fwd.length_squared() > 1e-5 {
                        fwd.normalize()
                    } else {
                        Vec3::NEG_Z
                    };
                }
            } else if cached.is_ultimate_tracker {
                snap.connected_tracker_count += 1;
                let m = pose.device_to_absolute.m;
                let pos = Vec3::new(m[3], m[7], m[11]);
                let speed = Vec3::from_array(pose.velocity.v).length();
                snap.trackers.push(TrackerReading {
                    serial: cached.serial.clone(),
                    pos,
                    linear_speed: speed,
                    pose_valid: pose.pose_is_valid,
                });

                if pose.pose_is_valid {
                    if snap.valid_tracker_count < 3 {
                        let k = snap.valid_tracker_count;
                        snap.solver_pos[k] = pos;
                        snap.solver_serials[k] = cached.serial.clone();
                        snap.solver_paths[k] = cached.device_path.clone();
                    }
                    snap.valid_tracker_count += 1;
                    if speed > snap.max_tracker_linear_speed {
                        snap.max_tracker_linear_speed = speed;
                    }
                }
            }
        }

        snap
    }

    fn get_string_property(&self, device_idx: u32, prop: i32) -> String {
        let mut stack_buf = [0u8; 256];
        let mut err = 0i32;
        let len = unsafe {
            (self.get_string_prop)(
                device_idx,
                prop,
                stack_buf.as_mut_ptr(),
                stack_buf.len() as u32,
                &mut err,
            )
        };
        if err == TRACKED_PROP_BUFFER_TOO_SMALL && len > 1 && len <= 4096 {
            let mut large = vec![0u8; len as usize];
            err = 0;
            let len2 = unsafe {
                (self.get_string_prop)(device_idx, prop, large.as_mut_ptr(), len, &mut err)
            };
            if err != 0 || len2 <= 1 {
                return String::new();
            }
            return c_bytes_to_string(&large[..(len2 as usize - 1).min(large.len())]);
        }
        if err != 0 || len <= 1 {
            return String::new();
        }
        c_bytes_to_string(&stack_buf[..(len as usize - 1).min(stack_buf.len())])
    }
}

impl Drop for OpenVrSession {
    fn drop(&mut self) {
        unsafe {
            (self.vr_shutdown)();
            FreeLibrary(self.module);
        }
    }
}

fn c_bytes_to_string(slice: &[u8]) -> String {
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

fn parse_overlay_table(p: *const usize) -> Result<OverlayApi, String> {
    Ok(OverlayApi {
        destroy_overlay: read_slot(p, 2)?,
        set_overlay_flag: read_slot(p, 10)?,
        set_overlay_width: read_slot(p, 21)?,
        set_overlay_texture_bounds: read_slot(p, 29)?,
        is_overlay_visible: read_slot(p, 43)?,
        poll_next_overlay_event: read_slot(p, 46)?,
        set_overlay_input_method: read_slot(p, 48)?,
        set_overlay_mouse_scale: read_slot(p, 50)?,
        set_overlay_texture: read_slot(p, 58)?,
        set_overlay_raw: read_slot(p, 60)?,
        create_dashboard_overlay: read_slot(p, 65)?,
        show_dashboard: read_slot(p, 70)?,
    })
}

fn read_slot<T: Copy>(table: *const usize, slot: usize) -> Result<T, String> {
    let addr = unsafe { *table.add(slot) };
    if addr == 0 {
        return Err(format!("Null OpenVR FnTable entry at slot {slot}"));
    }
    Ok(unsafe { std::mem::transmute_copy(&addr) })
}

fn load_symbol<T: Copy>(module: HMODULE, name_nul: &[u8]) -> Result<T, String> {
    let ptr = unsafe { GetProcAddress(module, name_nul.as_ptr()) };
    let Some(f) = ptr else {
        let name = String::from_utf8_lossy(&name_nul[..name_nul.len().saturating_sub(1)]);
        return Err(format!("Missing symbol {name} in openvr_api.dll"));
    };
    Ok(unsafe { std::mem::transmute_copy(&f) })
}

fn load_openvr_dll() -> Result<HMODULE, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("openvr_api.dll"));
            candidates.push(dir.join("lib").join("openvr_api.dll"));
        }
    }
    candidates.push(PathBuf::from("lib").join("openvr_api.dll"));
    candidates.push(PathBuf::from("openvr_api.dll"));

    for path in &candidates {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let h = unsafe { LoadLibraryW(wide.as_ptr()) };
        if !h.is_null() {
            return Ok(h);
        }
    }

    Err("Could not load openvr_api.dll (place it next to tracker_setup_lite.exe or in lib/)".to_string())
}
