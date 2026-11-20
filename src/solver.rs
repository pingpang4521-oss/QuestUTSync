use glam::{EulerRot, Quat, Vec2, Vec3};
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeltaTransform {
    pub yaw_radians: f32,
    pub translation: Vec3,
}

impl DeltaTransform {
    pub fn yaw_degrees(self) -> f32 {
        self.yaw_radians.to_degrees()
    }

    pub fn rotation(self) -> Quat {
        Quat::from_axis_angle(Vec3::Y, self.yaw_radians)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationTransform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}

impl Default for CalibrationTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl CalibrationTransform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: 1.0,
    };

    /// Returns `(yaw_deg, pitch_deg, roll_deg)` in YXZ intrinsic Euler order.
    pub fn to_ypr_degrees(self) -> (f32, f32, f32) {
        let (yaw, pitch, roll) = self.rotation.normalize().to_euler(EulerRot::YXZ);
        (yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees())
    }

    pub fn from_ypr_degrees(position: Vec3, yaw_deg: f32, pitch_deg: f32, roll_deg: f32, scale: f32) -> Self {
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            yaw_deg.to_radians(),
            pitch_deg.to_radians(),
            roll_deg.to_radians(),
        )
        .normalize();
        Self {
            position,
            rotation,
            scale: if scale > 0.0 { scale } else { 1.0 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandingIndices {
    pub waist: usize,
    pub foot_a: usize,
    pub foot_b: usize,
}

pub fn try_classify_standing_trackers(pos: &[Vec3; 3]) -> Option<StandingIndices> {
    let mut idx = [0usize, 1, 2];
    if pos[idx[0]].y < pos[idx[1]].y {
        idx.swap(0, 1);
    }
    if pos[idx[1]].y < pos[idx[2]].y {
        idx.swap(1, 2);
    }
    if pos[idx[0]].y < pos[idx[1]].y {
        idx.swap(0, 1);
    }

    let waist = idx[0];
    let foot_a = idx[1];
    let foot_b = idx[2];

    let waist_above_feet = pos[waist].y - pos[foot_a].y.max(pos[foot_b].y);
    let foot_height_diff = (pos[foot_a].y - pos[foot_b].y).abs();
    let foot_span_xz = Vec2::new(pos[foot_a].x, pos[foot_a].z)
        .distance(Vec2::new(pos[foot_b].x, pos[foot_b].z));

    if waist_above_feet >= 0.35 && foot_height_diff <= 0.12 && (0.06..=0.65).contains(&foot_span_xz) {
        Some(StandingIndices { waist, foot_a, foot_b })
    } else {
        None
    }
}

pub fn solve_delta_transform(
    hmd_pos: Vec3,
    hmd_forward_flat: Vec3,
    waist_pos: Vec3,
    left_foot_standing_pos: Vec3,
    right_foot_standing_pos: Vec3,
    feet_back_meters: f32,
) -> DeltaTransform {
    // 1. Vector from Right Foot to Left Foot in horizontal XZ plane.
    let rl_x = left_foot_standing_pos.x - right_foot_standing_pos.x;
    let rl_z = left_foot_standing_pos.z - right_foot_standing_pos.z;

    // Forward = normalize((Left - Right) x Up) = normalize(-rlZ, 0, rlX)
    let trk_forward = Vec3::new(-rl_z, 0.0, rl_x).normalize_or_zero();
    let qst_forward = Vec3::new(hmd_forward_flat.x, 0.0, hmd_forward_flat.z).normalize_or_zero();

    let angle_quest = qst_forward.x.atan2(qst_forward.z);
    let angle_trk = trk_forward.x.atan2(trk_forward.z);
    let delta_yaw = normalize_angle(angle_quest - angle_trk);

    let delta_rot = Quat::from_axis_angle(Vec3::Y, delta_yaw);

    // 2. Horizontal XZ uses 50/50 blend of Waist + Ankle midpoint to cancel posture lean; Y uses planted ankles.
    let current_ankle_mid = 0.5 * (left_foot_standing_pos + right_foot_standing_pos);
    let current_body_ref = Vec3::new(
        0.5 * (current_ankle_mid.x + waist_pos.x),
        current_ankle_mid.y,
        0.5 * (current_ankle_mid.z + waist_pos.z),
    );

    // ponytail: fixed anthropometric guess (ankles 6% of head height, feet_back_meters behind HMD); tune via slider/--feet-back-cm.
    let target_body_ref =
        Vec3::new(hmd_pos.x, hmd_pos.y * 0.06, hmd_pos.z) - qst_forward * feet_back_meters;
    let rotated_body_ref = delta_rot * current_body_ref;

    DeltaTransform {
        yaw_radians: delta_yaw,
        translation: target_body_ref - rotated_body_ref,
    }
}

pub fn compose_calibration(current: CalibrationTransform, delta: DeltaTransform) -> CalibrationTransform {
    let dr = delta.rotation();
    let new_rot = (dr * current.rotation).normalize();
    let new_pos = dr * current.position + delta.translation;
    CalibrationTransform {
        position: new_pos,
        rotation: new_rot,
        scale: if current.scale > 0.0 { current.scale } else { 1.0 },
    }
}

fn normalize_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

pub fn run_self_test() -> Result<(), String> {
    println!("Running QuestUTSync self-test (measurement-free)...");

    // 1. Test 4-DOF Leg-Lift Solver under a 135-degree rotated + shifted VIVE playspace
    let hmd_pos = Vec3::new(0.25, 1.60, -0.40);
    let hmd_yaw = 25.0_f32.to_radians();
    let hmd_forward = Vec3::new(hmd_yaw.sin(), 0.0, -hmd_yaw.cos());
    let hmd_right = Vec3::new(-hmd_forward.z, 0.0, hmd_forward.x);

    const FEET_BACK: f32 = 0.08;
    let true_ankle_center = Vec3::new(hmd_pos.x, hmd_pos.y * 0.06, hmd_pos.z) - hmd_forward * FEET_BACK;
    let true_left_foot = true_ankle_center - hmd_right * 0.13;
    let true_right_foot = true_ankle_center + hmd_right * 0.13;
    let true_waist = true_ankle_center + Vec3::new(0.0, 0.85, 0.0);

    let err_rot = Quat::from_axis_angle(Vec3::Y, -135.0_f32.to_radians());
    let err_shift = Vec3::new(1.80, -0.45, -2.30);

    let raw_left = err_rot.inverse() * (true_left_foot - err_shift);
    let raw_right = err_rot.inverse() * (true_right_foot - err_shift);
    let raw_waist = err_rot.inverse() * (true_waist - err_shift);

    let existing_cal = CalibrationTransform {
        position: Vec3::new(0.3, 0.1, -0.2),
        rotation: Quat::from_axis_angle(Vec3::Y, 0.4),
        scale: 1.0,
    };

    let steamvr_left = existing_cal.rotation * raw_left + existing_cal.position;
    let steamvr_right = existing_cal.rotation * raw_right + existing_cal.position;
    let steamvr_waist = existing_cal.rotation * raw_waist + existing_cal.position;

    let cls = try_classify_standing_trackers(&[steamvr_right, steamvr_waist, steamvr_left])
        .ok_or_else(|| "Failed to classify standing trackers".to_string())?;
    if cls.waist != 1 {
        return Err(format!("Waist must be classified as index 1, got {}", cls.waist));
    }

    let delta = solve_delta_transform(
        hmd_pos,
        hmd_forward,
        steamvr_waist,
        steamvr_left,
        steamvr_right,
        FEET_BACK,
    );
    let composed = compose_calibration(existing_cal, delta);

    let solved_left = composed.rotation * raw_left + composed.position;
    let solved_right = composed.rotation * raw_right + composed.position;

    let left_err = solved_left.distance(true_left_foot);
    let right_err = solved_right.distance(true_right_foot);
    if left_err > 1e-4 || right_err > 1e-4 {
        return Err(format!("Solver error too large: left={left_err}, right={right_err}"));
    }
    println!(
        "  [PASS] 4-DOF Leg-Lift solver recovered exact pose (residual error = {:.4} mm)",
        left_err.max(right_err) * 1000.0
    );

    // 1b. Verify compose_calibration preserves pitch/roll tilt present in Calibration.json
    let tilted_existing = CalibrationTransform {
        position: Vec3::new(0.3, 0.1, -0.2),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.4, 0.05, -0.04),
        scale: 1.0,
    };
    let tilted_raw_left = tilted_existing.rotation.inverse() * (steamvr_left - tilted_existing.position);
    let tilted_raw_right = tilted_existing.rotation.inverse() * (steamvr_right - tilted_existing.position);
    let composed_tilted = compose_calibration(tilted_existing, delta);
    let tilted_err = (composed_tilted.rotation * tilted_raw_left + composed_tilted.position)
        .distance(true_left_foot)
        .max(
            (composed_tilted.rotation * tilted_raw_right + composed_tilted.position)
                .distance(true_right_foot),
        );
    if tilted_err > 1e-4 {
        return Err(format!("Tilted composition error too large: {tilted_err}"));
    }
    println!(
        "  [PASS] 3D quaternion composition preserves existing tilt (residual error = {:.4} mm)",
        tilted_err * 1000.0
    );

    // 2. Test 52-byte binary packet layout for ViveTrackerServer.exe cmd 0x1F
    let pkt = crate::pipe::build_submit_transform_packet(composed, 1234, 5678);
    if pkt.len() != 52 {
        return Err(format!("Expected 52-byte packet, got {}", pkt.len()));
    }
    let magic = u32::from_le_bytes(pkt[0..4].try_into().unwrap());
    let cmd = u32::from_le_bytes(pkt[4..8].try_into().unwrap());
    let payload_len = u32::from_le_bytes(pkt[16..20].try_into().unwrap());
    if magic != 0x1111_1111 || cmd != 0x1F || payload_len != 32 {
        return Err("Header fields mismatch ViveTrackerServer.exe specification".to_string());
    }
    println!("  [PASS] 52-byte Submit_Space_Calibration_Transform (0x1F) packet verified.");

    // 3. Verify OpenVR C ABI struct sizes/offsets on Windows x64
    crate::openvr::verify_abi_layout()?;
    println!("  [PASS] OpenVR x64 ABI struct layout verified.");

    println!("All self-tests passed!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        run_self_test().expect("self-test failed");
    }

    #[test]
    fn ypr_roundtrip() {
        let cal = CalibrationTransform::from_ypr_degrees(Vec3::new(1.2, -0.3, 0.5), 42.0, 3.5, -2.25, 1.0);
        let (y, p, r) = cal.to_ypr_degrees();
        assert!((y - 42.0).abs() < 1e-3);
        assert!((p - 3.5).abs() < 1e-3);
        assert!((r + 2.25).abs() < 1e-3);
    }
}
