# QuestUTSync

Standalone **HTC Vive Ultimate Tracker** manager & hands-free playspace synchronizer for inside-out VR headsets (Meta Quest Pro, Quest 3/3S, Quest 2) in SteamVR — written in Rust with a SpaceCalibrator-inspired desktop UI and native SteamVR Dashboard Overlay.

Launch **without opening HTC VIVE Hub**: QuestUTSync silently starts `ViveTrackerServer.exe` in standalone mode, lets you **pair trackers** to dongle slots 1–5, run a **hands-free audio-guided room scan**, and **calibrate your playspace in 2 seconds** with a single leg lift.

---

## Highlights

- **Silent Standalone Backend (No HTC App Needed):** Automatically detects and silently launches `ViveTrackerServer.exe` in its built-in standalone mode (no VIVE Hub `VHConsole.exe` window) and shuts it down cleanly on exit.
- **Hands-Free Room Scan (`Scan Room` / `--scan`):** Replaces HTC's `tracker_setup.exe` VR arrow-chasing wizard. Start a scan with one click (Desktop or VR Dashboard) or `--scan`, slowly pan 1 tracker 360° around your room (no headset required), follow the live **8-sector coverage compass ring** and rising audio milestone/warning cues, and auto-save/sync the map to all trackers at your target quality (`50–90%`).
- **Built-In Dongle Pairing (`Pair Tracker` / `--pair`):** Pair new trackers directly to the VIVE Wireless Dongle with one click (auto-selects the next free slot `1–5`), or select a specific **Dongle Slot (1–5)** to **Pair Slot** / **Unpair** over `TrackerNamedPipe` without VIVE Hub.
- **Two-Second Hands-Free Playspace Sync:** Stand upright for 1 second, then lift your left leg (>10 cm). The 4-DOF closed-form solver computes yaw rotation and 3D translation and snaps all trackers into alignment — no figure-8 controller waving and no 4th tracker mounted to your headset.
- **5+ Tracker & Hand-Sleeve Ready (`SleeveJoyConVR` Bridge):** Exclude Left/Right Hand Sleeve trackers (`[L]` / `[R]`) from the 3-tracker FBT solver so 5+ Vive Ultimate Trackers (Waist + L/R Feet + L/R Hands) calibrate seamlessly alongside optional `SleeveJoyConVR` hand/Joy-Con controls.
- **SpaceCalibrator-Inspired UI (Desktop + VR Overlay):** Two-column Reference (Quest HMD) vs. Target (Vive Ultimate Trackers) live monitor, live leg-lift & room-scan progress bars, and a Manual Transform & Offset Editor (`X/Y/Z`, `Yaw/Pitch/Roll` nudge + Apply/Reload/Reset), accessible both on desktop and inside the SteamVR Dashboard via `IVROverlay`.
- **Automatic SteamVR Tracker Roles:** Assigns `TrackerRole_Waist`, `TrackerRole_LeftFoot`, and `TrackerRole_RightFoot` automatically in SteamVR settings for instant VRChat Full Body Tracking recognition.
- **Native Rust Binary:** Fast startup, minimal footprint, zero runtime dependencies beyond `openvr_api.dll`.

---

## Requirements

1. **VR Headset:** Meta Quest (Pro, 3, 3S, 2) connected to PC via Steam Link, Virtual Desktop, or AirLink.
2. **Trackers & Runtime:** HTC Vive Ultimate Trackers + wireless dongle, with the VIVE Hub runtime installed (provides `ViveTrackerServer.exe` — you do **not** need to open the VIVE Hub app). Keep 3 FBT trackers (waist + both feet) active for syncing, or mark any extra hand trackers with `[L]` / `[R]`.
3. **SteamVR:** Running on Windows 10/11 x64 for playspace calibration (room scan & pairing work even without SteamVR running).

---

## How to Use

### 1. Pair & Scan Your Room (First-Time Setup or New Lighting)
1. Plug in your VIVE Wireless Dongle and launch `tracker_setup_lite.exe` (or `Run-QuestUTSync.bat`). `Server: Standalone` will light up automatically.
2. *(If pairing a new tracker)* Click **Pair Tracker** (or choose a slot `1–5` and click **Pair Slot**), then hold the tracker's power button until its LED blinks blue.
3. Turn on at least 1 tracker and click **Scan Room** (or **Scan Room Now** if no map is stored). Slowly pan/turn the tracker 360° (low and high) while listening to the rising audio tones until it auto-finishes and syncs the map to all trackers.

### 2. Calibrate Playspace in VR (2 Seconds)
1. Launch SteamVR, put on your headset, and ensure your 3 FBT trackers are tracking. (If using 5 trackers with Hand Sleeves, click `[L]` / `[R]` next to your hand trackers to exclude them from FBT.)
2. In VR:
   - **Step 1:** Stand comfortably upright with both feet on the floor and look straight ahead for 1 second. You will hear a single audio tone indicating the detector is **Armed**.
   - **Step 2:** Lift your **left foot** upward by at least 10 cm (marching motion). You will hear a double audio tone indicating the playspace is **Synced**.
3. All trackers snap to your body in SteamVR and VRChat. Use the **Edit Calibration Transform** panel (desktop or SteamVR Dashboard **QuestUTSync** tab) to fine-tune `X/Y/Z` or `Yaw/Pitch/Roll` if desired.

> **Important:** It must be the **left** foot. The trackers alone cannot tell left from right, so lifting the right foot is accepted but rotates the result by 180°.
>
> A long low tone instead of the double tone means the calibration was sent but `ViveTrackerServer.exe` did not confirm it. Check the Activity Log and make sure the server pill is green.

---

## Command-Line Options

| Option | Description |
|---|---|
| `--scan` | Launch `ViveTrackerServer.exe` silently (if needed), run a hands-free audio-guided room scan, save & sync the map, and exit. |
| `--pair` | Launch `ViveTrackerServer.exe` silently (if needed), put the next free dongle slot (1–5) into pairing mode, and exit. |
| `--no-autolaunch` | Do not auto-launch `ViveTrackerServer.exe` silently on startup. |
| `--headless` | Run in console mode without opening the GUI or VR overlay. |
| `--once` | Perform one calibration cycle (headless) and terminate. |
| `--dry-run` | Calculate calibration transformation without writing to the named pipe. |
| `--feet-back-cm <n>` | Place your feet `n` cm behind the headset (default `0`, range −30 to 30). Also adjustable live via the UI slider. |
| `--selftest` | Execute internal solver, packet, and OpenVR ABI verification tests. |
| `--help`, `-h` | Display help screen. |

---

## Building from Source

Prerequisites: [Rust toolchain](https://rustup.rs/) (Windows x64).

```powershell
# Clone the repository
git clone https://github.com/pingpang4521-oss/QuestUTSync.git
cd QuestUTSync

# Run tests & build release binary
cargo test
cargo build --release

# Run verification test
.\target\release\tracker_setup_lite.exe --selftest
```

Place `lib\openvr_api.dll` alongside `tracker_setup_lite.exe` (or keep `lib\` next to it) when distributing.

---

## Technical Details

- **4-DOF Closed-Form Solver:** Inside-out VR headsets and Vive Ultimate Trackers both align vertically with gravity through onboard IMUs. Calibration solves 1 rotational degree of freedom (yaw $\theta$ around the vertical axis) and 3 translational degrees of freedom ($X, Y, Z$).
- **Direct Named-Pipe IPC (`\\.\pipe\TrackerNamedPipe`):** Communicates directly with `ViveTrackerServer.exe` for space calibration (`0x1E` / `0x1F`), room map creation & coverage/light/speed telemetry (`0x21` / `0x22` / `0x26` / `0x27` / `0x28` / `0x2C` / `0x30` / `0x34`), tracker enumeration (`0x17`), and dongle pairing/unpairing (`0x1B` / `0x1C` via the `VSWNamedPipeService` query envelope).
- **Process Alias:** Compiled as `tracker_setup_lite.exe` (and auto-relaunching under that name if renamed) to conform to `ViveTrackerServer.exe`'s named pipe process identification checks.
- **OpenVR Integration:** Dynamically loads `openvr_api.dll` using `IVRSystem_022`, `IVRSettings_003`, and `IVROverlay_027` (`VRApplication_Overlay` for GUI mode, `VRApplication_Background` for headless mode), rendering the shared `egui` interface to both the desktop window and an offscreen OpenGL FBO submitted to the SteamVR Dashboard.

---

## License

Distributed under the [MIT License](LICENSE).

`lib/openvr_api.dll` is Valve's OpenVR runtime library, redistributed under the BSD 3-Clause license (see [lib/openvr_api.LICENSE.txt](lib/openvr_api.LICENSE.txt)).
