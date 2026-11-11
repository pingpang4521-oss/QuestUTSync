# QuestUTSync

Automated, hands-free playspace synchronization between HTC Vive Ultimate Trackers and inside-out VR headsets (Meta Quest Pro, Quest 3, Quest 2) in SteamVR.

---

## Highlights

- **Hands-Free Calibration:** No figure-8 controller waving, and no 4th tracker mounted to your headset.
- **Two-Second Sync:** Stand upright for 1 second, then lift your left leg. The solver computes the 4-DOF transformation (yaw rotation and 3D translation) and snaps the trackers into place.
- **Direct VIVE Hub Injection:** Transmits calibration matrices directly to `ViveTrackerServer.exe` via its local IPC named pipe (`\\.\pipe\TrackerNamedPipe`, command `0x1F`). No custom SteamVR driver installation or driver hook required.
- **Automatic Tracker Roles:** Sets `TrackerRole_Waist`, `TrackerRole_LeftFoot`, and `TrackerRole_RightFoot` in SteamVR settings automatically so VRChat Full Body Tracking recognises tracker assignments immediately.
- **Self-Contained:** Zero external runtime dependencies. Runs as a lightweight single executable.

---

## Requirements

1. **VR Headset:** Meta Quest (Pro, 3, 3S, 2) connected to PC via Steam Link, Virtual Desktop, or AirLink.
2. **Trackers:** 3x HTC Vive Ultimate Trackers paired and tracking in official VIVE Hub software.
3. **SteamVR:** Running on Windows 10/11 x64.
4. **Runtime:** .NET 8 runtime (if building from source).

---

## How to Use

1. Launch SteamVR on your PC and put on your headset.
2. Turn on your 3 Vive Ultimate Trackers and confirm they are tracking in VIVE Hub.
3. Run `tracker_setup_lite.exe` (or launch `Run-QuestUTSync.bat`).
4. In VR:
   - **Step 1:** Stand comfortably upright with both feet on the floor and look straight ahead for 1 second. You will hear a single audio tone indicating the detector is **Armed**.
   - **Step 2:** Lift your **left foot** upward by at least 10 cm (marching motion). You will hear a double audio tone indicating the playspace is **Synced**.
5. All three trackers will snap to your body in SteamVR and in VRChat.

---

## Command-Line Options

| Option | Description |
|---|---|
| `--once` | Perform one calibration cycle and terminate. |
| `--dry-run` | Calculate calibration transformation without writing to the named pipe. |
| `--selftest` | Execute internal solver verification tests. |
| `--help`, `-h` | Display help screen. |

---

## Building from Source

Prerequisites: [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0).

```powershell
# Clone the repository
git clone https://github.com/pingpang4521-oss/QuestUTSync.git
cd QuestUTSync

# Build standalone distribution
dotnet publish -c Release -o dist QuestUTSync.csproj

# Run verification test
.\dist\tracker_setup_lite.exe --selftest
```

---

## Technical Details

- **4-DOF Closed-Form Solver:** Inside-out VR headsets and Vive Ultimate Trackers both align vertically with gravity through onboard IMUs. Calibration solves 1 rotational degree of freedom (yaw $\theta$ around the vertical axis) and 3 translational degrees of freedom ($X, Y, Z$).
- **Process Alias:** The executable is compiled under the assembly name `tracker_setup_lite.exe` to conform to VIVE Hub's internal named pipe process identification requirements.
- **SteamVR OpenVR Reader:** Connects via `openvr_api.dll` as a background VR application (`VRApplication_Background`) using `IVRSystem` and `IVRSettings`.

---

## License

Distributed under the [MIT License](LICENSE).
