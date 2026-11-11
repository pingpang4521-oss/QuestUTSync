using System.Buffers.Binary;
using System.Diagnostics;
using System.IO.Pipes;
using System.Numerics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json.Nodes;

namespace QuestUTSync;

internal static class Program
{
    private static readonly string DefaultViveCalibPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "HTC", "ViveSoftware", "ViveUltimateTracker", "config", "Calibration.json");
    private const string PipeName = "TrackerNamedPipe";

    public static int Main(string[] args)
    {
        Console.Title = "QuestUTSync (tracker_setup_lite pipe alias)";
        bool showHelp = args.Contains("--help", StringComparer.OrdinalIgnoreCase) ||
                        args.Contains("-h", StringComparer.OrdinalIgnoreCase) ||
                        args.Contains("/?", StringComparer.OrdinalIgnoreCase) ||
                        args.Contains("-?");
        if (showHelp)
        {
            PrintHelp();
            return 0;
        }

        bool runOnce = args.Contains("--once", StringComparer.OrdinalIgnoreCase);
        bool selfTest = args.Contains("--selftest", StringComparer.OrdinalIgnoreCase);
        bool dryRun = args.Contains("--dry-run", StringComparer.OrdinalIgnoreCase);

        if (selfTest)
        {
            return RunSelfTest();
        }

        Console.WriteLine("============================================================");
        Console.WriteLine(" QuestUTSync - Automatic Vive Ultimate Tracker Calibration");
        Console.WriteLine(" Process Alias : " + Process.GetCurrentProcess().ProcessName + ".exe");
        Console.WriteLine("============================================================");

        if (!OpenVrReader.TryInitialize(out var vr, out string vrErr))
        {
            Console.ForegroundColor = ConsoleColor.Red;
            Console.WriteLine($"[OpenVR] Init failed: {vrErr}");
            Console.ResetColor();
            Console.WriteLine("Make sure SteamVR is running, then launch QuestUTSync again.");
            return 1;
        }

        try
        {
            RunCalibrationLoop(vr!, runOnce, dryRun);
            return 0;
        }
        finally
        {
            vr?.Dispose();
        }
    }

    private static void RunCalibrationLoop(OpenVrReader vr, bool runOnce, bool dryRun)
    {
        Console.WriteLine("\n[Ready] How to sync at any time in VR:");
        Console.WriteLine("  1. Stand comfortably & look straight ahead for 1 second (1st beep = Armed).");
        Console.WriteLine("  2. Lift your LEFT leg up ~10+ cm while keeping your right foot planted (double beep = Synced!).\n");

        const int PollHz = 30;
        const float StillVelocityThreshold = 0.04f; // 4 cm/s max velocity while standing still
        const float StillAngularThreshold = 0.12f;  // ~7 deg/s max HMD angular velocity while standing still
        const float StillDurationRequired = 1.0f;   // 1.0s of stillness to arm
        const float ArmedTimeoutSeconds = 4.5f;     // window to lift left foot after arming
        const float LiftHeightThreshold = 0.10f;    // 10 cm vertical rise on lifted left foot
        const float PlantedMaxMove = 0.035f;        // 3.5 cm max movement on planted right foot
        const float HeadMaxMoveDuringLift = 0.08f;  // 8 cm max head movement during leg lift

        Vector3 prevHmdPos = default;
        Vector3[] prevTrkPos = new Vector3[3];
        bool hasPrev = false;
        long lastTick = Stopwatch.GetTimestamp();

        float stillTimer = 0f;
        int stillSampleCount = 0;
        Vector3 sumHmdPos = default;
        Vector3 sumHmdForward = default;
        Vector3[] sumTrkPos = new Vector3[3];

        bool isArmed = false;
        float armedTimer = 0f;
        DateTime cooldownUntil = DateTime.MinValue;

        StandingSnapshot baseline = new() { TrackerPos = new Vector3[3] };
        int lastDeviceCountLog = -1;

        while (true)
        {
            Thread.Sleep(1000 / PollHz);

            if (DateTime.UtcNow < cooldownUntil)
            {
                hasPrev = false;
                stillTimer = 0f;
                stillSampleCount = 0;
                isArmed = false;
                continue;
            }

            if (!vr.TryGetSnapshot(out var frame))
            {
                if (lastDeviceCountLog != frame.TrackerCount)
                {
                    Console.WriteLine($"[Waiting] Found HMD={frame.HasHmd}, Ultimate Trackers={frame.TrackerCount}/3...");
                    lastDeviceCountLog = frame.TrackerCount;
                }
                hasPrev = false;
                stillTimer = 0f;
                stillSampleCount = 0;
                isArmed = false;
                continue;
            }

            if (lastDeviceCountLog != 3)
            {
                Console.ForegroundColor = ConsoleColor.Green;
                Console.WriteLine($"[Tracked] HMD ({frame.HmdSerial}) + 3 Ultimate Trackers ({frame.TrackerSerials[0]}, {frame.TrackerSerials[1]}, {frame.TrackerSerials[2]}) active.");
                Console.ResetColor();
                lastDeviceCountLog = 3;
            }

            long nowTick = Stopwatch.GetTimestamp();
            float dt = Math.Clamp((float)Stopwatch.GetElapsedTime(lastTick, nowTick).TotalSeconds, 0.001f, 0.25f);
            lastTick = nowTick;

            if (!hasPrev)
            {
                prevHmdPos = frame.HmdPos;
                Array.Copy(frame.TrackerPos, prevTrkPos, 3);
                hasPrev = true;
                continue;
            }

            float hmdSpeed = frame.HmdLinearSpeed > 0f
                ? frame.HmdLinearSpeed
                : Vector3.Distance(frame.HmdPos, prevHmdPos) / dt;
            float maxTrkSpeed = frame.MaxTrackerLinearSpeed;
            if (maxTrkSpeed <= 0f)
            {
                for (int i = 0; i < 3; i++)
                {
                    float s = Vector3.Distance(frame.TrackerPos[i], prevTrkPos[i]) / dt;
                    if (s > maxTrkSpeed) maxTrkSpeed = s;
                }
            }

            prevHmdPos = frame.HmdPos;
            Array.Copy(frame.TrackerPos, prevTrkPos, 3);

            if (!isArmed)
            {
                if (hmdSpeed < StillVelocityThreshold &&
                    frame.HmdAngularSpeed < StillAngularThreshold &&
                    maxTrkSpeed < StillVelocityThreshold &&
                    LegLiftSolver.TryClassifyStandingTrackers(frame.TrackerPos, out int waistIdx, out int footAIdx, out int footBIdx))
                {
                    if (stillSampleCount == 0)
                    {
                        sumHmdPos = Vector3.Zero;
                        sumHmdForward = Vector3.Zero;
                        Array.Clear(sumTrkPos, 0, 3);
                    }

                    stillTimer += dt;
                    stillSampleCount++;
                    sumHmdPos += frame.HmdPos;
                    sumHmdForward += frame.HmdForwardFlat;
                    for (int i = 0; i < 3; i++) sumTrkPos[i] += frame.TrackerPos[i];

                    if (stillTimer >= StillDurationRequired)
                    {
                        float invN = 1.0f / stillSampleCount;
                        Vector3 avgFwd = sumHmdForward * invN;
                        avgFwd = avgFwd.LengthSquared() > 1e-5f ? Vector3.Normalize(avgFwd) : frame.HmdForwardFlat;

                        for (int i = 0; i < 3; i++) baseline.TrackerPos[i] = sumTrkPos[i] * invN;

                        LegLiftSolver.TryClassifyStandingTrackers(baseline.TrackerPos, out waistIdx, out footAIdx, out footBIdx);

                        baseline.HmdPos = sumHmdPos * invN;
                        baseline.HmdForwardFlat = avgFwd;
                        baseline.WaistIdx = waistIdx;
                        baseline.FootAIdx = footAIdx;
                        baseline.FootBIdx = footBIdx;

                        isArmed = true;
                        armedTimer = 0f;
                        stillTimer = 0f;
                        stillSampleCount = 0;
                        Console.ForegroundColor = ConsoleColor.Cyan;
                        Console.WriteLine("[Armed] Standing baseline locked! Lift your LEFT leg up (>10 cm) now...");
                        Console.ResetColor();
                        TryBeepAsync(660, 120);
                    }
                }
                else
                {
                    stillTimer = 0f;
                    stillSampleCount = 0;
                }
            }
            else
            {
                armedTimer += dt;
                if (armedTimer > ArmedTimeoutSeconds)
                {
                    Console.WriteLine("[Timeout] No leg lift detected within 4.5s. Returning to standby.");
                    isArmed = false;
                    continue;
                }

                float hmdDrift = Vector2.Distance(
                    new Vector2(frame.HmdPos.X, frame.HmdPos.Z),
                    new Vector2(baseline.HmdPos.X, baseline.HmdPos.Z));
                if (hmdDrift > HeadMaxMoveDuringLift)
                {
                    Console.WriteLine("[Reset] Head moved during leg-lift window. Stand still to re-arm.");
                    isArmed = false;
                    continue;
                }

                Vector3 posA = frame.TrackerPos[baseline.FootAIdx];
                Vector3 posB = frame.TrackerPos[baseline.FootBIdx];
                Vector3 baseA = baseline.TrackerPos[baseline.FootAIdx];
                Vector3 baseB = baseline.TrackerPos[baseline.FootBIdx];

                float riseA = posA.Y - baseA.Y;
                float riseB = posB.Y - baseB.Y;
                float moveA = Vector3.Distance(posA, baseA);
                float moveB = Vector3.Distance(posB, baseB);

                int liftedIdx = -1;
                int plantedIdx = -1;

                if (riseA >= LiftHeightThreshold && moveB <= PlantedMaxMove)
                {
                    liftedIdx = baseline.FootAIdx;
                    plantedIdx = baseline.FootBIdx;
                }
                else if (riseB >= LiftHeightThreshold && moveA <= PlantedMaxMove)
                {
                    liftedIdx = baseline.FootBIdx;
                    plantedIdx = baseline.FootAIdx;
                }

                if (liftedIdx != -1)
                {
                    Console.ForegroundColor = ConsoleColor.Yellow;
                    Console.WriteLine($"[Leg Lift Detected!] Left Foot = {frame.TrackerSerials[liftedIdx]} (+{MathF.Max(riseA, riseB) * 100f:F1} cm), " +
                                      $"Right Foot = {frame.TrackerSerials[plantedIdx]}, Waist = {frame.TrackerSerials[baseline.WaistIdx]}");
                    Console.ResetColor();

                    var currentCal = VivePipeClient.ReadCurrentCalibration(DefaultViveCalibPath);
                    var delta = LegLiftSolver.SolveDeltaTransform(
                        baseline.HmdPos,
                        baseline.HmdForwardFlat,
                        baseline.TrackerPos[baseline.WaistIdx],
                        baseline.TrackerPos[liftedIdx],
                        baseline.TrackerPos[plantedIdx]);

                    var newCal = LegLiftSolver.ComposeCalibration(currentCal, delta);

                    Console.WriteLine($"[Solver] Delta Yaw = {delta.YawDegrees:F2} deg, Delta Pos = ({delta.Translation.X:F3}, {delta.Translation.Y:F3}, {delta.Translation.Z:F3})");
                    Console.WriteLine($"[Solver] New Global Transform -> Pos=({newCal.Position.X:F3}, {newCal.Position.Y:F3}, {newCal.Position.Z:F3}), " +
                                      $"RotXYZW=({newCal.Rotation.X:F4}, {newCal.Rotation.Y:F4}, {newCal.Rotation.Z:F4}, {newCal.Rotation.W:F4})");

                    if (!dryRun)
                    {
                        vr.AssignTrackerRoles(
                            frame.TrackerDevicePaths[baseline.WaistIdx],
                            frame.TrackerDevicePaths[liftedIdx],
                            frame.TrackerDevicePaths[plantedIdx]);

                        if (VivePipeClient.TrySendCalibration(PipeName, newCal, out string pipeMsg))
                        {
                            Console.ForegroundColor = ConsoleColor.Green;
                            Console.WriteLine($"[Synced!] {pipeMsg}");
                            Console.ResetColor();
                            TryBeepAsync(880, 110, 1320, 160);
                        }
                        else
                        {
                            Console.ForegroundColor = ConsoleColor.Red;
                            Console.WriteLine($"[Pipe Warning] {pipeMsg}");
                            Console.ResetColor();
                        }
                    }

                    if (runOnce)
                    {
                        return;
                    }

                    isArmed = false;
                    cooldownUntil = DateTime.UtcNow.AddSeconds(8);
                    Console.WriteLine("[Cooldown] Sync complete. Pausing detector for 8 seconds...\n");
                }
            }
        }
    }

    private static void TryBeepAsync(int freq1, int ms1, int freq2 = 0, int ms2 = 0)
    {
        ThreadPool.QueueUserWorkItem(_ =>
        {
            try
            {
                if (OperatingSystem.IsWindows())
                {
                    Console.Beep(freq1, ms1);
                    if (freq2 > 0 && ms2 > 0)
                    {
                        Console.Beep(freq2, ms2);
                    }
                }
            }
            catch
            {
                // Ignore if no audio output device is attached to console
            }
        });
    }

    private static void PrintHelp()
    {
        Console.WriteLine("============================================================");
        Console.WriteLine(" QuestUTSync - Automatic Vive Ultimate Tracker Calibration");
        Console.WriteLine("============================================================");
        Console.WriteLine("Usage: tracker_setup_lite.exe [options]\n");
        Console.WriteLine("Options:");
        Console.WriteLine("  --once      Calibrate once and exit immediately");
        Console.WriteLine("  --dry-run   Compute alignment without sending to VIVE Hub pipe");
        Console.WriteLine("  --selftest  Run built-in verification math test suite");
        Console.WriteLine("  --help, -h  Show this help screen\n");
        Console.WriteLine("In-VR Calibration Procedure:");
        Console.WriteLine("  1. Stand upright and look straight ahead for 1s (Single beep = Armed).");
        Console.WriteLine("  2. Lift your LEFT foot up 10+ cm (Double beep = Synced!).");
    }

    private static int RunSelfTest()
    {
        Console.WriteLine("Running QuestUTSync self-test (measurement-free)...");

        // 1. Test 4-DOF Leg-Lift Solver under a 135-degree rotated + shifted VIVE playspace
        Vector3 hmdPos = new(0.25f, 1.60f, -0.40f);
        float hmdYaw = 25f * MathF.PI / 180f;
        Vector3 hmdForward = new(MathF.Sin(hmdYaw), 0f, -MathF.Cos(hmdYaw));
        Vector3 hmdRight = new(-hmdForward.Z, 0f, hmdForward.X);

        // In SteamVR Standing space (Y=0 floor), ankles sit at ~6% of HMD height above floor, directly under HMD XZ
        Vector3 trueAnkleCenter = new(hmdPos.X, hmdPos.Y * 0.06f, hmdPos.Z);
        Vector3 trueLeftFoot = trueAnkleCenter - hmdRight * 0.13f;
        Vector3 trueRightFoot = trueAnkleCenter + hmdRight * 0.13f;
        Vector3 trueWaist = trueAnkleCenter + new Vector3(0f, 0.85f, 0f);

        float errYaw = -135f * MathF.PI / 180f;
        Quaternion errRot = Quaternion.CreateFromAxisAngle(Vector3.UnitY, errYaw);
        Vector3 errShift = new(1.80f, -0.45f, -2.30f);

        Vector3 rawLeft = Vector3.Transform(trueLeftFoot - errShift, Quaternion.Inverse(errRot));
        Vector3 rawRight = Vector3.Transform(trueRightFoot - errShift, Quaternion.Inverse(errRot));
        Vector3 rawWaist = Vector3.Transform(trueWaist - errShift, Quaternion.Inverse(errRot));

        var existingCal = new CalibrationTransform(
            new Vector3(0.3f, 0.1f, -0.2f),
            Quaternion.CreateFromAxisAngle(Vector3.UnitY, 0.4f),
            1.0f);

        Vector3 steamVrLeft = Vector3.Transform(rawLeft, existingCal.Rotation) + existingCal.Position;
        Vector3 steamVrRight = Vector3.Transform(rawRight, existingCal.Rotation) + existingCal.Position;
        Vector3 steamVrWaist = Vector3.Transform(rawWaist, existingCal.Rotation) + existingCal.Position;

        bool classified = LegLiftSolver.TryClassifyStandingTrackers(
            new[] { steamVrRight, steamVrWaist, steamVrLeft },
            out int wIdx, out int fA, out int fB);
        Debug.Assert(classified && wIdx == 1, "Waist must be classified as index 1 (highest Y)");

        var delta = LegLiftSolver.SolveDeltaTransform(hmdPos, hmdForward, steamVrWaist, steamVrLeft, steamVrRight);
        var composed = LegLiftSolver.ComposeCalibration(existingCal, delta);

        Vector3 solvedLeft = Vector3.Transform(rawLeft, composed.Rotation) + composed.Position;
        Vector3 solvedRight = Vector3.Transform(rawRight, composed.Rotation) + composed.Position;

        float leftErr = Vector3.Distance(solvedLeft, trueLeftFoot);
        float rightErr = Vector3.Distance(solvedRight, trueRightFoot);
        if (leftErr > 1e-4f || rightErr > 1e-4f)
        {
            throw new Exception($"Solver error too large: leftErr={leftErr}, rightErr={rightErr}");
        }
        Console.WriteLine($"  [PASS] 4-DOF Leg-Lift solver recovered exact pose (residual error = {MathF.Max(leftErr, rightErr) * 1000f:F4} mm)");

        // 2. Test 52-byte binary packet layout for ViveTrackerServer.exe cmd 0x1F
        byte[] pkt = VivePipeClient.BuildSubmitTransformPacket(composed, 1234, 5678);
        if (pkt.Length != 52)
        {
            throw new Exception($"Expected 52-byte packet, got {pkt.Length}");
        }
        uint magic = BinaryPrimitives.ReadUInt32LittleEndian(pkt.AsSpan(0, 4));
        uint cmd = BinaryPrimitives.ReadUInt32LittleEndian(pkt.AsSpan(4, 4));
        uint payloadLen = BinaryPrimitives.ReadUInt32LittleEndian(pkt.AsSpan(16, 4));
        if (magic != 0x11111111u || cmd != 0x1Fu || payloadLen != 32u)
        {
            throw new Exception("Header fields mismatch ViveTrackerServer.exe specification");
        }
        Console.WriteLine("  [PASS] 52-byte Submit_Space_Calibration_Transform (0x1F) packet verified.");
        Console.WriteLine("All self-tests passed!");
        return 0;
    }
}

internal struct StandingSnapshot
{
    public Vector3 HmdPos;
    public Vector3 HmdForwardFlat;
    public Vector3[] TrackerPos;
    public int WaistIdx;
    public int FootAIdx;
    public int FootBIdx;
}

internal readonly record struct DeltaTransform(float YawRadians, Vector3 Translation)
{
    public float YawDegrees => YawRadians * (180f / MathF.PI);
    public Quaternion Rotation => Quaternion.CreateFromAxisAngle(Vector3.UnitY, YawRadians);
}

internal readonly record struct CalibrationTransform(Vector3 Position, Quaternion Rotation, float Scale);

internal static class LegLiftSolver
{
    public static bool TryClassifyStandingTrackers(Vector3[] pos, out int waistIdx, out int footAIdx, out int footBIdx)
    {
        waistIdx = 0;
        footAIdx = 1;
        footBIdx = 2;
        if (pos.Length < 3) return false;

        int i0 = 0, i1 = 1, i2 = 2;
        if (pos[i0].Y < pos[i1].Y) (i0, i1) = (i1, i0);
        if (pos[i1].Y < pos[i2].Y) (i1, i2) = (i2, i1);
        if (pos[i0].Y < pos[i1].Y) (i0, i1) = (i1, i0);

        waistIdx = i0;
        footAIdx = i1;
        footBIdx = i2;

        float waistAboveFeet = pos[waistIdx].Y - MathF.Max(pos[footAIdx].Y, pos[footBIdx].Y);
        float footHeightDiff = MathF.Abs(pos[footAIdx].Y - pos[footBIdx].Y);
        float footSpanXz = Vector2.Distance(
            new Vector2(pos[footAIdx].X, pos[footAIdx].Z),
            new Vector2(pos[footBIdx].X, pos[footBIdx].Z));

        return waistAboveFeet >= 0.35f && footHeightDiff <= 0.12f && footSpanXz >= 0.06f && footSpanXz <= 0.65f;
    }

    public static DeltaTransform SolveDeltaTransform(
        Vector3 hmdPos,
        Vector3 hmdForwardFlat,
        Vector3 waistPos,
        Vector3 leftFootStandingPos,
        Vector3 rightFootStandingPos)
    {
        // 1. Vector from Right Foot to Left Foot in horizontal XZ plane
        float rlX = leftFootStandingPos.X - rightFootStandingPos.X;
        float rlZ = leftFootStandingPos.Z - rightFootStandingPos.Z;

        // Forward = normalize((Left - Right) x Up) = normalize(-rlZ, 0, rlX)
        Vector3 trkForward = Vector3.Normalize(new Vector3(-rlZ, 0f, rlX));
        Vector3 qstForward = Vector3.Normalize(new Vector3(hmdForwardFlat.X, 0f, hmdForwardFlat.Z));

        float angleQuest = MathF.Atan2(qstForward.X, qstForward.Z);
        float angleTrk = MathF.Atan2(trkForward.X, trkForward.Z);
        float deltaYaw = NormalizeAngle(angleQuest - angleTrk);

        Quaternion deltaRot = Quaternion.CreateFromAxisAngle(Vector3.UnitY, deltaYaw);

        // 2. Horizontal XZ uses 50/50 blend of Waist + Ankle midpoint to cancel posture lean; Y uses planted ankles
        Vector3 currentAnkleMid = 0.5f * (leftFootStandingPos + rightFootStandingPos);
        Vector3 currentBodyRef = new(
            0.5f * (currentAnkleMid.X + waistPos.X),
            currentAnkleMid.Y,
            0.5f * (currentAnkleMid.Z + waistPos.Z));

        Vector3 targetBodyRef = new(hmdPos.X, hmdPos.Y * 0.06f, hmdPos.Z);
        Vector3 rotatedBodyRef = Vector3.Transform(currentBodyRef, deltaRot);

        Vector3 deltaTranslation = targetBodyRef - rotatedBodyRef;
        return new DeltaTransform(deltaYaw, deltaTranslation);
    }

    public static CalibrationTransform ComposeCalibration(CalibrationTransform current, DeltaTransform delta)
    {
        Quaternion dR = delta.Rotation;
        Quaternion newRot = Quaternion.Normalize(Quaternion.Multiply(dR, current.Rotation));

        newRot = new Quaternion(0f, newRot.Y, 0f, newRot.W);
        newRot = Quaternion.Normalize(newRot);

        Vector3 newPos = Vector3.Transform(current.Position, dR) + delta.Translation;
        return new CalibrationTransform(newPos, newRot, current.Scale > 0f ? current.Scale : 1.0f);
    }

    private static float NormalizeAngle(float a)
    {
        while (a > MathF.PI) a -= 2f * MathF.PI;
        while (a < -MathF.PI) a += 2f * MathF.PI;
        return a;
    }
}

internal static class VivePipeClient
{
    private const uint ClientToServerMagic = 0x11111111u;
    private const uint CmdNotifyStartSpaceCalibration = 0x1Eu; // 30
    private const uint CmdSubmitSpaceCalibrationTransform = 0x1Fu; // 31

    public static CalibrationTransform ReadCurrentCalibration(string calibJsonPath)
    {
        try
        {
            if (File.Exists(calibJsonPath))
            {
                var root = JsonNode.Parse(File.ReadAllText(calibJsonPath));
                var tf = root?["transform"];
                if (tf != null)
                {
                    var posArr = tf["position"]?.AsArray();
                    var rotArr = tf["rotation"]?.AsArray();
                    float scale = tf["scale"]?.GetValue<float>() ?? 1.0f;

                    if (posArr != null && posArr.Count == 3 && rotArr != null && rotArr.Count == 4)
                    {
                        Vector3 pos = new(
                            posArr[0]!.GetValue<float>(),
                            posArr[1]!.GetValue<float>(),
                            posArr[2]!.GetValue<float>());
                        Quaternion rot = new(
                            rotArr[0]!.GetValue<float>(),
                            rotArr[1]!.GetValue<float>(),
                            rotArr[2]!.GetValue<float>(),
                            rotArr[3]!.GetValue<float>());
                        if (rot.LengthSquared() > 1e-6f)
                        {
                            return new CalibrationTransform(pos, Quaternion.Normalize(rot), scale > 0f ? scale : 1.0f);
                        }
                    }
                }
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Calibration.json] Read warning: {ex.Message}");
        }

        return new CalibrationTransform(Vector3.Zero, Quaternion.Identity, 1.0f);
    }

    public static byte[] BuildHeaderPacket(uint cmd, uint pid, uint tid, uint payloadLen)
    {
        byte[] buf = new byte[20];
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(0, 4), ClientToServerMagic);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(4, 4), cmd);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(8, 4), pid);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(12, 4), tid);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(16, 4), payloadLen);
        return buf;
    }

    public static byte[] BuildSubmitTransformPacket(CalibrationTransform cal, uint pid, uint tid)
    {
        byte[] buf = new byte[52];
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(0, 4), ClientToServerMagic);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(4, 4), CmdSubmitSpaceCalibrationTransform);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(8, 4), pid);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(12, 4), tid);
        BinaryPrimitives.WriteUInt32LittleEndian(buf.AsSpan(16, 4), 32u);

        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(20, 4), cal.Position.X);
        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(24, 4), cal.Position.Y);
        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(28, 4), cal.Position.Z);

        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(32, 4), cal.Rotation.X);
        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(36, 4), cal.Rotation.Y);
        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(40, 4), cal.Rotation.Z);
        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(44, 4), cal.Rotation.W);

        BinaryPrimitives.WriteSingleLittleEndian(buf.AsSpan(48, 4), cal.Scale);
        return buf;
    }

    public static bool TrySendCalibration(string pipeName, CalibrationTransform cal, out string statusMessage)
    {
        uint pid = (uint)Environment.ProcessId;
        uint tid = (uint)Environment.CurrentManagedThreadId;
        byte[] startPkt = BuildHeaderPacket(CmdNotifyStartSpaceCalibration, pid, tid, 0);
        byte[] submitPkt = BuildSubmitTransformPacket(cal, pid, tid);
        byte[] ackBuf = new byte[256];

        Exception? lastErr = null;
        for (int attempt = 1; attempt <= 3; attempt++)
        {
            try
            {
                using var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
                pipe.Connect(800);

                pipe.Write(startPkt, 0, startPkt.Length);
                pipe.Flush();
                _ = pipe.ReadAsync(ackBuf, 0, ackBuf.Length).Wait(120);

                pipe.Write(submitPkt, 0, submitPkt.Length);
                pipe.Flush();
                var readTask = pipe.ReadAsync(ackBuf, 0, ackBuf.Length);
                bool gotAck = readTask.Wait(250) && readTask.Result >= 8 &&
                              BinaryPrimitives.ReadUInt32LittleEndian(ackBuf.AsSpan(4, 4)) == CmdSubmitSpaceCalibrationTransform;

                statusMessage = gotAck
                    ? "ViveTrackerServer ACKed 52-byte Submit_Space_Calibration_Transform (0x1F)."
                    : "Sent 52-byte Submit_Space_Calibration_Transform (0x1F) to \\\\.\\pipe\\TrackerNamedPipe.";
                return true;
            }
            catch (Exception ex)
            {
                lastErr = ex;
                if (attempt < 3) Thread.Sleep(50);
            }
        }

        statusMessage = $"Failed to send to \\\\.\\pipe\\{pipeName}: {lastErr?.Message}";
        return false;
    }
}

internal struct FrameSnapshot
{
    public bool HasHmd;
    public string HmdSerial;
    public Vector3 HmdPos;
    public Vector3 HmdForwardFlat;
    public float HmdLinearSpeed;
    public float HmdAngularSpeed;
    public float MaxTrackerLinearSpeed;
    public int TrackerCount;
    public string[] TrackerSerials;
    public string[] TrackerDevicePaths;
    public Vector3[] TrackerPos;
}

internal sealed unsafe class OpenVrReader : IDisposable
{
    private const uint MaxTrackedDevices = 64;
    private const int TrackingUniverseStanding = 1;
    private const int TrackedDeviceClassHmd = 1;
    private const int TrackedDeviceClassGenericTracker = 3;
    private const int PropSerialNumberString = 1002;
    private const int PropControllerTypeString = 1031;
    private const int PropRegisteredDeviceTypeString = 1037;

    [DllImport("openvr_api.dll", CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    private static extern uint VR_InitInternal2(ref int peError, int eApplicationType, string? pStartupInfo);

    [DllImport("openvr_api.dll", CallingConvention = CallingConvention.Cdecl)]
    private static extern void VR_ShutdownInternal();

    [DllImport("openvr_api.dll", CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    private static extern IntPtr VR_GetGenericInterface(string pchInterfaceVersion, ref int peError);

    [StructLayout(LayoutKind.Sequential)]
    private struct HmdMatrix34_t
    {
        public float m0, m1, m2, m3;
        public float m4, m5, m6, m7;
        public float m8, m9, m10, m11;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HmdVector3_t
    {
        public float v0, v1, v2;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct TrackedDevicePose_t
    {
        public HmdMatrix34_t mDeviceToAbsoluteTracking;
        public HmdVector3_t vVelocity;
        public HmdVector3_t vAngularVelocity;
        public int eTrackingResult;
        [MarshalAs(UnmanagedType.I1)]
        public bool bPoseIsValid;
        [MarshalAs(UnmanagedType.I1)]
        public bool bDeviceIsConnected;
    }

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate void GetDeviceToAbsoluteTrackingPoseDelegate(
        int eOrigin,
        float fPredictedSecondsToPhotonsFromNow,
        [Out] TrackedDevicePose_t[] pTrackedDevicePoseArray,
        uint unTrackedDevicePoseArrayCount);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetTrackedDeviceClassDelegate(uint unDeviceIndex);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate uint GetStringTrackedDevicePropertyDelegate(
        uint unDeviceIndex,
        int prop,
        StringBuilder pchValue,
        uint unBufferSize,
        ref int pError);

    [UnmanagedFunctionPointer(CallingConvention.StdCall, CharSet = CharSet.Ansi)]
    private delegate void SetSettingsStringDelegate(
        string pchSection,
        string pchSettingsKey,
        string pchValue,
        ref int peError);

    private struct CachedDevice
    {
        public bool IsEvaluated;
        public bool IsHmd;
        public bool IsUltimateTracker;
        public string Serial;
        public string DevicePath;
    }

    private readonly GetDeviceToAbsoluteTrackingPoseDelegate _getPoses;
    private readonly GetTrackedDeviceClassDelegate _getDeviceClass;
    private readonly GetStringTrackedDevicePropertyDelegate _getStringProp;
    private readonly SetSettingsStringDelegate? _setSettingsString;
    private readonly TrackedDevicePose_t[] _poseBuffer = new TrackedDevicePose_t[MaxTrackedDevices];
    private readonly CachedDevice[] _deviceCache = new CachedDevice[MaxTrackedDevices];
    private readonly StringBuilder _propSb = new(128);
    private readonly string[] _snapshotSerials = new string[3];
    private readonly string[] _snapshotPaths = new string[3];
    private readonly Vector3[] _snapshotPos = new Vector3[3];
    private bool _disposed;

    private OpenVrReader(
        GetDeviceToAbsoluteTrackingPoseDelegate getPoses,
        GetTrackedDeviceClassDelegate getDeviceClass,
        GetStringTrackedDevicePropertyDelegate getStringProp,
        SetSettingsStringDelegate? setSettingsString)
    {
        _getPoses = getPoses;
        _getDeviceClass = getDeviceClass;
        _getStringProp = getStringProp;
        _setSettingsString = setSettingsString;
    }

    public static bool TryInitialize(out OpenVrReader? reader, out string error)
    {
        reader = null;
        int err = 0;
        VR_InitInternal2(ref err, 3, null);
        if (err != 0)
        {
            error = $"VR_InitInternal2 returned EVRInitError={err}";
            return false;
        }

        string[] versions = { "FnTable:IVRSystem_022", "FnTable:IVRSystem_021", "FnTable:IVRSystem_020" };
        IntPtr fnTable = IntPtr.Zero;
        foreach (string ver in versions)
        {
            err = 0;
            fnTable = VR_GetGenericInterface(ver, ref err);
            if (fnTable != IntPtr.Zero && err == 0)
            {
                break;
            }
        }

        if (fnTable == IntPtr.Zero)
        {
            VR_ShutdownInternal();
            error = $"Failed to acquire IVRSystem FnTable (err={err})";
            return false;
        }

        IntPtr pGetPoses = Marshal.ReadIntPtr(fnTable, 11 * IntPtr.Size);
        IntPtr pGetClass = Marshal.ReadIntPtr(fnTable, 19 * IntPtr.Size);
        IntPtr pGetString = Marshal.ReadIntPtr(fnTable, 27 * IntPtr.Size);

        SetSettingsStringDelegate? setSettingsStr = null;
        int sErr = 0;
        IntPtr settingsTable = VR_GetGenericInterface("FnTable:IVRSettings_003", ref sErr);
        if (settingsTable != IntPtr.Zero && sErr == 0)
        {
            IntPtr pSetStr = Marshal.ReadIntPtr(settingsTable, 4 * IntPtr.Size);
            if (pSetStr != IntPtr.Zero)
            {
                setSettingsStr = Marshal.GetDelegateForFunctionPointer<SetSettingsStringDelegate>(pSetStr);
            }
        }

        reader = new OpenVrReader(
            Marshal.GetDelegateForFunctionPointer<GetDeviceToAbsoluteTrackingPoseDelegate>(pGetPoses),
            Marshal.GetDelegateForFunctionPointer<GetTrackedDeviceClassDelegate>(pGetClass),
            Marshal.GetDelegateForFunctionPointer<GetStringTrackedDevicePropertyDelegate>(pGetString),
            setSettingsStr);

        error = string.Empty;
        return true;
    }

    public void AssignTrackerRoles(string waistPath, string leftFootPath, string rightFootPath)
    {
        if (_setSettingsString == null) return;
        int err = 0;
        if (!string.IsNullOrEmpty(waistPath))
            _setSettingsString("trackers", waistPath, "TrackerRole_Waist", ref err);
        if (!string.IsNullOrEmpty(leftFootPath))
            _setSettingsString("trackers", leftFootPath, "TrackerRole_LeftFoot", ref err);
        if (!string.IsNullOrEmpty(rightFootPath))
            _setSettingsString("trackers", rightFootPath, "TrackerRole_RightFoot", ref err);

        Console.WriteLine($"[SteamVR Roles] Assigned Waist={waistPath}, LeftFoot={leftFootPath}, RightFoot={rightFootPath}");
    }

    public bool TryGetSnapshot(out FrameSnapshot snapshot)
    {
        snapshot = new FrameSnapshot
        {
            HmdSerial = string.Empty,
            TrackerSerials = _snapshotSerials,
            TrackerDevicePaths = _snapshotPaths,
            TrackerPos = _snapshotPos
        };

        _getPoses(TrackingUniverseStanding, 0f, _poseBuffer, MaxTrackedDevices);

        for (uint i = 0; i < MaxTrackedDevices; i++)
        {
            ref var pose = ref _poseBuffer[i];
            ref var cached = ref _deviceCache[i];

            if (!pose.bDeviceIsConnected)
            {
                cached.IsEvaluated = false;
                continue;
            }

            if (!pose.bPoseIsValid) continue;

            if (!cached.IsEvaluated)
            {
                int devClass = _getDeviceClass(i);
                string serial = GetStringProperty(i, PropSerialNumberString);
                bool isHmd = devClass == TrackedDeviceClassHmd;
                bool isUt = false;
                string devPath = string.Empty;

                if (devClass == TrackedDeviceClassGenericTracker)
                {
                    string ctrlType = GetStringProperty(i, PropControllerTypeString);
                    isUt =
                        serial.StartsWith("48-A", StringComparison.OrdinalIgnoreCase) ||
                        serial.StartsWith("43A", StringComparison.OrdinalIgnoreCase) ||
                        ctrlType.Contains("ultimate", StringComparison.OrdinalIgnoreCase);

                    if (isUt)
                    {
                        string regPath = GetStringProperty(i, PropRegisteredDeviceTypeString);
                        devPath = !string.IsNullOrEmpty(regPath)
                            ? (regPath.StartsWith('/') ? regPath : "/devices/" + regPath)
                            : $"/devices/vive_ultimate_tracker/{serial}";
                    }
                }

                cached = new CachedDevice
                {
                    IsEvaluated = true,
                    IsHmd = isHmd,
                    IsUltimateTracker = isUt,
                    Serial = serial,
                    DevicePath = devPath
                };
            }

            if (cached.IsHmd && !snapshot.HasHmd)
            {
                ref var m = ref pose.mDeviceToAbsoluteTracking;
                snapshot.HasHmd = true;
                snapshot.HmdSerial = cached.Serial;
                snapshot.HmdPos = new Vector3(m.m3, m.m7, m.m11);
                snapshot.HmdLinearSpeed = new Vector3(pose.vVelocity.v0, pose.vVelocity.v1, pose.vVelocity.v2).Length();
                snapshot.HmdAngularSpeed = new Vector3(pose.vAngularVelocity.v0, pose.vAngularVelocity.v1, pose.vAngularVelocity.v2).Length();

                Vector3 fwd = new(-m.m2, 0f, -m.m10);
                snapshot.HmdForwardFlat = fwd.LengthSquared() > 1e-5f
                    ? Vector3.Normalize(fwd)
                    : new Vector3(0f, 0f, -1f);
            }
            else if (cached.IsUltimateTracker && snapshot.TrackerCount < 3)
            {
                ref var m = ref pose.mDeviceToAbsoluteTracking;
                int idx = snapshot.TrackerCount++;
                snapshot.TrackerSerials[idx] = cached.Serial;
                snapshot.TrackerDevicePaths[idx] = cached.DevicePath;
                snapshot.TrackerPos[idx] = new Vector3(m.m3, m.m7, m.m11);
                float trkSpeed = new Vector3(pose.vVelocity.v0, pose.vVelocity.v1, pose.vVelocity.v2).Length();
                if (trkSpeed > snapshot.MaxTrackerLinearSpeed)
                    snapshot.MaxTrackerLinearSpeed = trkSpeed;
            }
        }

        return snapshot.HasHmd && snapshot.TrackerCount == 3;
    }

    private string GetStringProperty(uint deviceIdx, int prop)
    {
        _propSb.Clear();
        int err = 0;
        uint len = _getStringProp(deviceIdx, prop, _propSb, (uint)_propSb.Capacity, ref err);
        return err == 0 && len > 0 ? _propSb.ToString() : string.Empty;
    }

    public void Dispose()
    {
        if (!_disposed)
        {
            _disposed = true;
            VR_ShutdownInternal();
        }
    }
}
