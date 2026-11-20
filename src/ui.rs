use crate::detector::{CalibrationEngine, DetectorPhase, LogLevel, SyncResultKind};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};

const BG_DARK: Color32 = Color32::from_rgb(18, 21, 27);
const BG_CARD: Color32 = Color32::from_rgb(26, 30, 39);
const BG_SUBTLE: Color32 = Color32::from_rgb(33, 38, 49);
const BORDER: Color32 = Color32::from_rgb(52, 60, 76);
const TEXT_MUTED: Color32 = Color32::from_rgb(148, 163, 184);
const ACCENT_CYAN: Color32 = Color32::from_rgb(56, 189, 248);
const ACCENT_GREEN: Color32 = Color32::from_rgb(52, 211, 153);
const ACCENT_AMBER: Color32 = Color32::from_rgb(251, 191, 36);
const ACCENT_RED: Color32 = Color32::from_rgb(248, 113, 113);

pub struct UiActions {
    pub reconnect_vr: bool,
    pub show_in_dashboard: bool,
}

pub fn apply_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = BG_DARK;
    visuals.panel_fill = BG_DARK;
    visuals.faint_bg_color = BG_SUBTLE;
    visuals.extreme_bg_color = Color32::from_rgb(13, 16, 21);
    visuals.widgets.noninteractive.bg_fill = BG_CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.bg_fill = BG_SUBTLE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 52, 68);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    visuals.widgets.active.bg_fill = Color32::from_rgb(30, 64, 90);
    visuals.selection.bg_fill = Color32::from_rgb(20, 83, 115);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    ctx.set_style(style);
}

pub fn draw(
    ctx: &egui::Context,
    engine: &mut CalibrationEngine,
    overlay_ready: bool,
    in_vr_overlay: bool,
) -> UiActions {
    let mut actions = UiActions {
        reconnect_vr: false,
        show_in_dashboard: false,
    };

    egui::TopBottomPanel::top("header")
        .frame(
            egui::Frame::new()
                .fill(BG_CARD)
                .inner_margin(Margin::symmetric(14, 10))
                .stroke(Stroke::new(1.0_f32, BORDER)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("QuestUTSync")
                        .strong()
                        .size(18.0)
                        .color(Color32::WHITE),
                );
                pill(
                    ui,
                    if in_vr_overlay {
                        "VR Dashboard"
                    } else {
                        "tracker_setup_lite.exe"
                    },
                    Color32::from_rgb(40, 48, 64),
                    TEXT_MUTED,
                );

                ui.add_space(6.0);

                // SteamVR pill
                let vr_ok = engine.vr.is_some();
                pill(
                    ui,
                    if vr_ok { "SteamVR: Connected" } else { "SteamVR: Offline" },
                    if vr_ok {
                        Color32::from_rgb(16, 62, 48)
                    } else {
                        Color32::from_rgb(76, 26, 30)
                    },
                    if vr_ok { ACCENT_GREEN } else { ACCENT_RED },
                );

                // Overlay pill
                if !in_vr_overlay {
                    pill(
                        ui,
                        if overlay_ready { "Overlay: Active" } else { "Overlay: Off" },
                        if overlay_ready {
                            Color32::from_rgb(15, 58, 78)
                        } else {
                            BG_SUBTLE
                        },
                        if overlay_ready { ACCENT_CYAN } else { TEXT_MUTED },
                    );
                }

                // Tracker count pill
                let cnt = engine.latest_frame.connected_tracker_count;
                let (t_bg, t_fg) = if cnt == 3 && engine.latest_frame.valid_tracker_count == 3 {
                    (Color32::from_rgb(16, 62, 48), ACCENT_GREEN)
                } else if cnt > 3 {
                    (Color32::from_rgb(76, 26, 30), ACCENT_RED)
                } else {
                    (Color32::from_rgb(72, 54, 16), ACCENT_AMBER)
                };
                pill(ui, &format!("Trackers: {cnt}/3"), t_bg, t_fg);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !vr_ok {
                        if ui.button("Connect SteamVR").clicked() {
                            actions.reconnect_vr = true;
                        }
                    } else if !in_vr_overlay && overlay_ready {
                        if ui.button("Open in VR").clicked() {
                            actions.show_in_dashboard = true;
                        }
                    }
                    ui.checkbox(&mut engine.dry_run, "Dry Run");
                    ui.checkbox(&mut engine.audio_beeps, "Audio");
                });
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(BG_DARK).inner_margin(Margin::same(12)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // 1. Two-column SpaceCalibrator-style Device Spaces
                ui.columns(2, |cols| {
                    draw_reference_space(&mut cols[0], engine);
                    draw_target_space(&mut cols[1], engine);
                });

                ui.add_space(10.0);

                // 2. Main Hands-Free Leg-Lift Calibration Card
                draw_calibration_card(ui, engine);

                ui.add_space(10.0);

                // 3. SpaceCalibrator-style Live Transform & Manual Offset Editor
                draw_transform_editor(ui, engine);

                ui.add_space(10.0);

                // 4. Activity Log
                draw_log_panel(ui, engine);
            });
        });

    actions
}

fn draw_reference_space(ui: &mut egui::Ui, engine: &CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Reference Space")
                    .strong()
                    .size(14.0)
                    .color(ACCENT_CYAN),
            );
            ui.label(RichText::new("Quest / HMD").size(12.0).color(TEXT_MUTED));
        });
        ui.separator();

        let f = &engine.latest_frame;
        if engine.vr.is_none() {
            ui.label(
                RichText::new(
                    engine
                        .vr_error
                        .as_deref()
                        .unwrap_or("Start SteamVR to read headset poses."),
                )
                .color(TEXT_MUTED),
            );
            return;
        }

        if !f.has_connected_hmd {
            ui.label(RichText::new("No headset detected in SteamVR.").color(ACCENT_AMBER));
            return;
        }

        let status_color = if f.has_hmd { ACCENT_GREEN } else { ACCENT_AMBER };
        let status_text = if f.has_hmd { "Tracking" } else { "Pose Invalid" };
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(status_color));
            ui.label(
                RichText::new(if f.hmd_serial.is_empty() {
                    "HMD"
                } else {
                    &f.hmd_serial
                })
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(status_text).size(12.0).color(status_color));
            });
        });

        ui.add_space(4.0);
        if f.has_hmd {
            let yaw_deg = f.hmd_forward_flat.x.atan2(-f.hmd_forward_flat.z).to_degrees();
            egui::Grid::new("hmd_stats")
                .num_columns(2)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Position (m)").color(TEXT_MUTED).size(12.0));
                    ui.monospace(format!(
                        "{:+.3}, {:+.3}, {:+.3}",
                        f.hmd_pos.x, f.hmd_pos.y, f.hmd_pos.z
                    ));
                    ui.end_row();

                    ui.label(RichText::new("Facing Yaw").color(TEXT_MUTED).size(12.0));
                    ui.monospace(format!("{:+.1}°", yaw_deg));
                    ui.end_row();

                    ui.label(RichText::new("Motion").color(TEXT_MUTED).size(12.0));
                    let still = f.hmd_linear_speed < 0.04 && f.hmd_angular_speed < 0.12;
                    ui.label(
                        RichText::new(format!(
                            "{:.2} m/s  •  {:.1}°/s",
                            f.hmd_linear_speed,
                            f.hmd_angular_speed.to_degrees()
                        ))
                        .monospace()
                        .size(12.0)
                        .color(if still { ACCENT_GREEN } else { TEXT_MUTED }),
                    );
                    ui.end_row();
                });
        }
    });
}

fn draw_target_space(ui: &mut egui::Ui, engine: &CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Target Space")
                    .strong()
                    .size(14.0)
                    .color(ACCENT_GREEN),
            );
            ui.label(
                RichText::new("Vive Ultimate Trackers")
                    .size(12.0)
                    .color(TEXT_MUTED),
            );
        });
        ui.separator();

        let f = &engine.latest_frame;
        if engine.vr.is_none() {
            ui.label(RichText::new("Waiting for SteamVR connection...").color(TEXT_MUTED));
            return;
        }

        if f.trackers.is_empty() {
            ui.label(
                RichText::new("No Vive Ultimate Trackers detected. Turn on 3 trackers in VIVE Hub.")
                    .color(ACCENT_AMBER),
            );
            return;
        }

        if f.connected_tracker_count > 3 {
            ui.label(
                RichText::new(format!(
                    "⚠ {} Ultimate Trackers connected — turn extra trackers off (need exactly 3).",
                    f.connected_tracker_count
                ))
                .size(12.0)
                .color(ACCENT_RED),
            );
            ui.add_space(4.0);
        }

        for trk in &f.trackers {
            let dot_col = if trk.pose_valid {
                ACCENT_GREEN
            } else {
                ACCENT_AMBER
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(dot_col));
                ui.label(RichText::new(&trk.serial).monospace().size(12.5));
                if let Some(&role) = engine.assigned_roles.get(&trk.serial) {
                    pill(ui, role, Color32::from_rgb(20, 62, 82), ACCENT_CYAN);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if trk.pose_valid {
                        let still_col = if trk.linear_speed < 0.04 {
                            ACCENT_GREEN
                        } else {
                            TEXT_MUTED
                        };
                        ui.label(
                            RichText::new(format!("{:.2} m/s", trk.linear_speed))
                                .monospace()
                                .size(11.5)
                                .color(still_col),
                        );
                        ui.label(
                            RichText::new(format!("Y {:.2}m", trk.pos.y))
                                .monospace()
                                .size(12.0)
                                .color(TEXT_MUTED),
                        );
                    } else {
                        ui.label(RichText::new("No Pose").size(12.0).color(ACCENT_AMBER));
                    }
                });
            });
        }
    });
}

fn draw_calibration_card(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Hands-Free Leg-Lift Calibration")
                    .strong()
                    .size(15.0)
                    .color(Color32::WHITE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut engine.auto_sync, "Auto-Sync Enabled");
                if engine.sync_count > 0 {
                    ui.label(
                        RichText::new(format!("Syncs: {}", engine.sync_count))
                            .size(12.0)
                            .color(ACCENT_GREEN),
                    );
                }
            });
        });

        ui.add_space(6.0);

        // Status banner + progress bar
        let (banner_bg, banner_fg, title, detail, progress) = match &engine.phase {
            DetectorPhase::Disconnected => (
                BG_SUBTLE,
                TEXT_MUTED,
                "SteamVR Disconnected".to_string(),
                "Start SteamVR (or click Connect SteamVR above). Auto-retrying every 2s.".to_string(),
                None,
            ),
            DetectorPhase::Paused => (
                BG_SUBTLE,
                TEXT_MUTED,
                "Auto-Sync Paused".to_string(),
                "Enable Auto-Sync or use the Manual Transform Editor below.".to_string(),
                None,
            ),
            DetectorPhase::WaitingForDevices => (
                Color32::from_rgb(48, 39, 18),
                ACCENT_AMBER,
                format!(
                    "Waiting for HMD + 3 Trackers ({}/3)",
                    engine.latest_frame.connected_tracker_count
                ),
                "Put on your headset and confirm all 3 trackers are tracking in VIVE Hub.".to_string(),
                None,
            ),
            DetectorPhase::StandStill => (
                Color32::from_rgb(20, 38, 52),
                ACCENT_CYAN,
                "Ready — Stand Upright & Look Straight Ahead".to_string(),
                "Step 1: Hold still for 1.0s to arm.  Step 2: Lift your LEFT foot (>10 cm).".to_string(),
                Some((0.0, "Hold still (0%)".to_string(), ACCENT_CYAN)),
            ),
            DetectorPhase::HoldingStill { progress } => (
                Color32::from_rgb(16, 54, 72),
                ACCENT_CYAN,
                "Locking Standing Baseline...".to_string(),
                "Keep standing still and looking straight ahead...".to_string(),
                Some((*progress, format!("Arming... {:.0}%", progress * 100.0), ACCENT_CYAN)),
            ),
            DetectorPhase::Armed {
                remaining_s,
                max_rise_cm,
            } => (
                Color32::from_rgb(12, 72, 68),
                ACCENT_GREEN,
                format!("ARMED — Lift Your LEFT Foot Now! ({remaining_s:.1}s)"),
                "Keep right foot planted. Do NOT lift your right foot (it flips yaw 180°).".to_string(),
                Some((
                    (*max_rise_cm / 10.0).clamp(0.0, 1.0),
                    format!("Foot rise: {max_rise_cm:.1} / 10.0 cm"),
                    ACCENT_GREEN,
                )),
            ),
            DetectorPhase::Cooldown {
                remaining_s,
                last_status,
            } => {
                let (bg, fg, label) = match last_status {
                    SyncResultKind::Synced => (
                        Color32::from_rgb(16, 62, 48),
                        ACCENT_GREEN,
                        format!("SYNCED! (Cooldown {remaining_s:.1}s)"),
                    ),
                    SyncResultKind::Unconfirmed => (
                        Color32::from_rgb(72, 52, 16),
                        ACCENT_AMBER,
                        format!("SENT — UNCONFIRMED BY VIVE HUB (Cooldown {remaining_s:.1}s)"),
                    ),
                    SyncResultKind::DryRun => (
                        Color32::from_rgb(24, 48, 72),
                        ACCENT_CYAN,
                        format!("DRY RUN COMPUTED (Cooldown {remaining_s:.1}s)"),
                    ),
                    SyncResultKind::Failed => (
                        Color32::from_rgb(76, 26, 30),
                        ACCENT_RED,
                        format!("PIPE SEND FAILED (Cooldown {remaining_s:.1}s)"),
                    ),
                };
                let delta_str = engine
                    .last_delta
                    .map(|d| {
                        format!(
                            "Delta Yaw: {:+.2}°  •  Delta Pos: ({:+.3}, {:+.3}, {:+.3}) m",
                            d.yaw_degrees(),
                            d.translation.x,
                            d.translation.y,
                            d.translation.z
                        )
                    })
                    .unwrap_or_default();
                (bg, fg, label, delta_str, None)
            }
        };

        egui::Frame::new()
            .fill(banner_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 10))
            .stroke(Stroke::new(1.0_f32, banner_fg.linear_multiply(0.5)))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).strong().size(15.0).color(banner_fg));
                if !detail.is_empty() {
                    ui.label(RichText::new(detail).size(12.5).color(Color32::from_rgb(220, 228, 240)));
                }
                if let Some((frac, text, col)) = progress {
                    ui.add_space(5.0);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .text(text)
                            .fill(col)
                            .desired_height(18.0),
                    );
                }
            });

        ui.add_space(8.0);

        // Tuning & quick action row
        ui.horizontal(|ui| {
            ui.label(RichText::new("Feet Back Offset:").color(TEXT_MUTED));
            ui.add(
                egui::Slider::new(&mut engine.feet_back_cm, -30.0..=30.0)
                    .suffix(" cm")
                    .step_by(0.5),
            );
            if ui.small_button("0").on_hover_text("Reset offset to 0 cm").clicked() {
                engine.feet_back_cm = 0.0;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Reset Detector").clicked() {
                    engine.disarm();
                }
            });
        });
    });
}

fn draw_transform_editor(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Edit Calibration Transform")
                    .strong()
                    .size(14.0)
                    .color(Color32::WHITE),
            );
            if engine.editor_dirty {
                pill(
                    ui,
                    "Modified (not applied)",
                    Color32::from_rgb(72, 52, 16),
                    ACCENT_AMBER,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Reset to Identity").clicked() {
                    engine.reset_calibration_to_identity();
                }
                if ui.button("Reload Calibration.json").clicked() {
                    engine.reload_calibration_from_disk();
                }
                let apply_btn = egui::Button::new(
                    RichText::new("Apply to VIVE Hub").strong().color(Color32::BLACK),
                )
                .fill(ACCENT_CYAN);
                if ui.add(apply_btn).clicked() {
                    engine.apply_editor_calibration();
                }
            });
        });

        ui.separator();

        let mut changed = false;
        ui.columns(2, |cols| {
            cols[0].label(RichText::new("Translation (meters)").size(12.0).color(TEXT_MUTED));
            egui::Grid::new("pos_grid")
                .num_columns(4)
                .spacing([6.0, 4.0])
                .show(&mut cols[0], |ui| {
                    for (i, label) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.label(RichText::new(*label).strong());
                        if ui.small_button("-1cm").clicked() {
                            engine.editor_pos[i] -= 0.01;
                            changed = true;
                        }
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut engine.editor_pos[i])
                                    .speed(0.002)
                                    .fixed_decimals(3)
                                    .suffix(" m"),
                            )
                            .changed();
                        if ui.small_button("+1cm").clicked() {
                            engine.editor_pos[i] += 0.01;
                            changed = true;
                        }
                        ui.end_row();
                    }
                });

            cols[1].label(RichText::new("Rotation (degrees)").size(12.0).color(TEXT_MUTED));
            egui::Grid::new("rot_grid")
                .num_columns(4)
                .spacing([6.0, 4.0])
                .show(&mut cols[1], |ui| {
                    for (i, label) in ["Yaw", "Pitch", "Roll"].iter().enumerate() {
                        ui.label(RichText::new(*label).strong());
                        if ui.small_button("-1°").clicked() {
                            engine.editor_ypr[i] -= 1.0;
                            changed = true;
                        }
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut engine.editor_ypr[i])
                                    .speed(0.1)
                                    .range(-180.0..=180.0)
                                    .fixed_decimals(2)
                                    .suffix("°"),
                            )
                            .changed();
                        if ui.small_button("+1°").clicked() {
                            engine.editor_ypr[i] += 1.0;
                            changed = true;
                        }
                        ui.end_row();
                    }
                });
        });

        if changed {
            engine.editor_dirty = true;
        }
    });
}

fn draw_log_panel(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Activity Log").strong().size(13.5).color(Color32::WHITE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    engine.logs.clear();
                }
            });
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(130.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if engine.logs.is_empty() {
                    ui.label(RichText::new("No events yet.").size(12.0).color(TEXT_MUTED));
                }
                for entry in &engine.logs {
                    let col = match entry.level {
                        LogLevel::Info => Color32::from_rgb(203, 213, 225),
                        LogLevel::Success => ACCENT_GREEN,
                        LogLevel::Accent => ACCENT_CYAN,
                        LogLevel::Warn => ACCENT_AMBER,
                        LogLevel::Error => ACCENT_RED,
                    };
                    ui.label(
                        RichText::new(format!("[{:6.1}s] {}", entry.elapsed_s, entry.message))
                            .monospace()
                            .size(11.5)
                            .color(col),
                    );
                }
            });
    });
}

fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(BG_CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

fn pill(ui: &mut egui::Ui, text: &str, fill: Color32, fg: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).strong().color(fg));
        });
}
