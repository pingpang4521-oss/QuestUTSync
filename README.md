# QuestUTSync

Automated, hands-free playspace synchronization between HTC Vive Ultimate Trackers and inside-out VR headsets (Meta Quest Pro, Quest 3, Quest 2) in SteamVR — written in Rust with a SpaceCalibrator-inspired desktop UI and native SteamVR Dashboard Overlay.

---

## Highlights

- **SpaceCalibrator-Inspired UI (Desktop + VR Overlay):** Two-column Reference (Quest HMD) vs. Target (Vive Ultimate Trackers) live monitor, hands-free leg-lift progress bar, and a live Manual Transform & Offset Editor (`X/Y/Z`, `Yaw/Pitch/Roll` nudge + Apply/Reload/Reset). Accessible both on desktop and inside the SteamVR Dashboard via `IVROverlay`.
- **Hands-Free Calibration:** No figure-8 controller waving, and no 4th tracker mounted to your headset.
- **Two-Second Sync:** Stand upright for 1 second, then lift your left leg. The solver computes the 4-DOF transformation (yaw rotation and 3D translation) and snaps the trackers into place.
- **Direct VIVE Hub Injection:** Transmits calibration matrices directly to `ViveTrackerServer.exe` via its local IPC named pipe (`\\.\pipe\TrackerNamedPipe`, command `0x1F`) with acknowledgement verification. No custom SteamVR driver installation or driver hook required.
- **Automatic Tracker Roles:** Sets `TrackerRole_Waist`, `TrackerRole_LeftFoot`, and `TrackerRole_RightFoot` in SteamVR settings automatically so VRChat Full Body Tracking recognizes tracker assignments immediately.
- **Native Rust Binary:** Fast startup, minimal footprint, zero runtime dependencies beyond `openvr_api.dll`.

---

## Requirements

1. **VR Headset:** Meta Quest (Pro, 3, 3S, 2) connected to PC via Steam Link, Virtual Desktop, or AirLink.
2. **Trackers:** Exactly 3 HTC Vive Ultimate Trackers (waist + both feet) paired and tracking in official VIVE Hub software. Turn any extra Ultimate Trackers off while syncing.
3. **SteamVR:** Running on Windows 10/11 x64.

---

## How to Use

1. Launch SteamVR on your PC and put on your headset.
2. Turn on your 3 Vive Ultimate Trackers and confirm they are tracking in VIVE Hub.
3. Run `tracker_setup_lite.exe` (or launch `Run-QuestUTSync.bat`). The desktop window opens immediately and registers a **QuestUTSync** tab inside the SteamVR Dashboard.
4. In VR:
   - **Step 1:** Stand comfortably upright with both feet on the floor and look straight ahead for 1 second. You will hear a single audio tone indicating the detector is **Armed**.
   - **Step 2:** Lift your **left foot** upward by at least 10 cm (marching motion). You will hear a double audio tone indicating the playspace is **Synced**.
5. All three trackers will snap to your body in SteamVR and in VRChat. Use the **Edit Calibration Transform** panel (on desktop or in the VR Dashboard tab) to fine-tune X/Y/Z or Yaw/Pitch/Roll if desired.

> **Important:** It must be the **left** foot. The trackers alone cannot tell left from right, so lifting the right foot is accepted but rotates the result by 180°.
>
> A long low tone instead of the double tone means the calibration was sent but VIVE Hub did not confirm it. Check the Activity Log, and make sure VIVE Hub is running.

---

## Command-Line Options

| Option | Description |
|---|---|
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
- **Process Alias:** Compiled as `tracker_setup_lite.exe` (and auto-relaunching under that name if renamed) to conform to VIVE Hub's internal named pipe process identification requirements.
- **OpenVR Integration:** Dynamically loads `openvr_api.dll` using `IVRSystem_022`, `IVRSettings_003`, and `IVROverlay_027` (`VRApplication_Overlay` for GUI mode, `VRApplication_Background` for headless mode), rendering the shared `egui` interface to both the desktop window and an offscreen OpenGL FBO submitted to the SteamVR Dashboard.

---

## License

Distributed under the [MIT License](LICENSE).

`lib/openvr_api.dll` is Valve's OpenVR runtime library, redistributed under the BSD 3-Clause license (see [lib/openvr_api.LICENSE.txt](lib/openvr_api.LICENSE.txt)).
