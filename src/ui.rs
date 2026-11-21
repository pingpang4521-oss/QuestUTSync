use crate::detector::{CalibrationEngine, DetectorPhase, LogLevel, SyncResultKind};
use crate::pipe::MapState;
use crate::scanner::{self, ScanPhase, ServerState};
use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, RichText, Stroke, Vec2};

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

                // VIVE Tracker Server pill
                let snap = &engine.scan_snapshot;
                let (srv_lbl, srv_bg, srv_fg) = match snap.server_state {
                    ServerState::Running if snap.launched_standalone => (
                        "Server: Standalone",
                        Color32::from_rgb(16, 62, 48),
                        ACCENT_GREEN,
                    ),
                    ServerState::Running => (
                        "Server: Online",
                        Color32::from_rgb(16, 62, 48),
                        ACCENT_GREEN,
                    ),
                    ServerState::Launching | ServerState::Checking => (
                        "Server: Starting...",
                        Color32::from_rgb(72, 54, 16),
                        ACCENT_AMBER,
                    ),
                    ServerState::Offline => (
                        "Server: Offline",
                        Color32::from_rgb(76, 26, 30),
                        ACCENT_RED,
                    ),
                    ServerState::NotInstalled => (
                        "Server: Missing",
                        Color32::from_rgb(76, 26, 30),
                        ACCENT_RED,
                    ),
                };
                pill(ui, srv_lbl, srv_bg, srv_fg);

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

                // Tracker count pill (FBT trackers + optional Hand Sleeve trackers)
                let cnt = engine.latest_frame.connected_tracker_count;
                let hand_cnt = engine.latest_frame.excluded_hand_tracker_count;
                let (t_bg, t_fg) = if cnt == 3 && engine.latest_frame.valid_tracker_count == 3 {
                    (Color32::from_rgb(16, 62, 48), ACCENT_GREEN)
                } else if cnt > 3 {
                    (Color32::from_rgb(76, 26, 30), ACCENT_RED)
                } else {
                    (Color32::from_rgb(72, 54, 16), ACCENT_AMBER)
                };
                pill(ui, &format!("FBT: {cnt}/3"), t_bg, t_fg);
                if hand_cnt > 0 {
                    pill(
                        ui,
                        &format!("Hands: {hand_cnt}"),
                        Color32::from_rgb(20, 56, 78),
                        ACCENT_CYAN,
                    );
                }

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

                // 2. Standalone Server & Hands-Free Room Scan Card
                draw_room_scan_card(ui, engine);

                ui.add_space(10.0);

                // 3. Main Hands-Free Leg-Lift Calibration Card
                draw_calibration_card(ui, engine);

                ui.add_space(10.0);

                // 3. Hand Sleeves + Switch Joy-Con (SleeveJoyConVR) Integration Card
                draw_sleeve_card(ui, engine);

                ui.add_space(10.0);

                // 4. SpaceCalibrator-style Live Transform & Manual Offset Editor
                draw_transform_editor(ui, engine);

                ui.add_space(10.0);

                // 5. Activity Log
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

fn draw_target_space(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Target Space")
                    .strong()
                    .size(14.0)
                    .color(ACCENT_GREEN),
            );
            ui.label(
                RichText::new("Vive Ultimate Trackers (click L/R to mark Hand Sleeves)")
                    .size(11.5)
                    .color(TEXT_MUTED),
            );
        });
        ui.separator();

        if engine.vr.is_none() {
            ui.label(RichText::new("Waiting for SteamVR connection...").color(TEXT_MUTED));
            return;
        }

        let trackers = engine.latest_frame.trackers.clone();
        let fbt_cnt = engine.latest_frame.connected_tracker_count;

        if trackers.is_empty() {
            ui.label(
                RichText::new("No Vive Ultimate Trackers detected. Turn on trackers in VIVE Hub.")
                    .color(ACCENT_AMBER),
            );
            return;
        }

        if fbt_cnt > 3 {
            ui.label(
                RichText::new(format!(
                    "⚠ {fbt_cnt} FBT trackers active (need 3). Click [L] / [R] below on your Hand Sleeve trackers to exclude them."
                ))
                .size(12.0)
                .color(ACCENT_RED),
            );
            ui.add_space(4.0);
        }

        let mut pending_toggle: Option<(bool, String)> = None;

        for trk in &trackers {
            let dot_col = if trk.pose_valid {
                ACCENT_GREEN
            } else {
                ACCENT_AMBER
            };
            let is_l = trk.hand_role == Some("L Hand");
            let is_r = trk.hand_role == Some("R Hand");

            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(dot_col));
                ui.label(RichText::new(&trk.serial).monospace().size(12.0));
                if let Some(hrole) = trk.hand_role {
                    pill(ui, hrole, Color32::from_rgb(35, 42, 82), ACCENT_CYAN);
                } else if let Some(&role) = engine.assigned_roles.get(&trk.serial) {
                    pill(ui, role, Color32::from_rgb(20, 62, 82), ACCENT_CYAN);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r_btn = egui::Button::new(RichText::new("R").size(11.0).strong().color(
                        if is_r { Color32::BLACK } else { TEXT_MUTED },
                    ))
                    .fill(if is_r { Color32::from_rgb(251, 113, 133) } else { BG_SUBTLE });
                    if ui
                        .add(r_btn)
                        .on_hover_text("Assign as Right Hand Sleeve tracker (exclude from FBT)")
                        .clicked()
                    {
                        pending_toggle = Some((false, trk.serial.clone()));
                    }

                    let l_btn = egui::Button::new(RichText::new("L").size(11.0).strong().color(
                        if is_l { Color32::BLACK } else { TEXT_MUTED },
                    ))
                    .fill(if is_l { ACCENT_CYAN } else { BG_SUBTLE });
                    if ui
                        .add(l_btn)
                        .on_hover_text("Assign as Left Hand Sleeve tracker (exclude from FBT)")
                        .clicked()
                    {
                        pending_toggle = Some((true, trk.serial.clone()));
                    }

                    if trk.pose_valid {
                        let still_col = if trk.linear_speed < 0.04 {
                            ACCENT_GREEN
                        } else {
                            TEXT_MUTED
                        };
                        ui.label(
                            RichText::new(format!("{:.2} m/s", trk.linear_speed))
                                .monospace()
                                .size(11.0)
                                .color(still_col),
                        );
                        ui.label(
                            RichText::new(format!("Y {:.2}m", trk.pos.y))
                                .monospace()
                                .size(11.5)
                                .color(TEXT_MUTED),
                        );
                    } else {
                        ui.label(RichText::new("No Pose").size(11.5).color(ACCENT_AMBER));
                    }
                });
            });
        }

        if let Some((is_left, serial)) = pending_toggle {
            match engine.sleeve.toggle_hand_serial(is_left, &serial) {
                Ok(msg) => engine.log(LogLevel::Accent, format!("[Sleeve] {msg}")),
                Err(e) => engine.log(LogLevel::Error, format!("[Sleeve] {e}")),
            }
        }
    });
}

fn draw_room_scan_card(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    let snap = engine.scan_snapshot.clone();
    let scanning_or_busy = matches!(
        snap.phase,
        ScanPhase::Starting | ScanPhase::Scanning { .. } | ScanPhase::Finalizing { .. }
    );

    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Room Scan & Tracker Pairing")
                    .strong()
                    .size(15.0)
                    .color(Color32::WHITE),
            );

            let (map_lbl, map_bg, map_fg) = match snap.server_state {
                ServerState::NotInstalled => (
                    "VIVE Runtime Not Found",
                    Color32::from_rgb(76, 26, 30),
                    ACCENT_RED,
                ),
                ServerState::Offline => (
                    "Server Offline",
                    Color32::from_rgb(76, 26, 30),
                    ACCENT_RED,
                ),
                ServerState::Launching | ServerState::Checking => (
                    "Starting Server...",
                    Color32::from_rgb(72, 54, 16),
                    ACCENT_AMBER,
                ),
                ServerState::Running => match snap.map_state {
                    MapState::Ready => (
                        "Map: Ready",
                        Color32::from_rgb(16, 62, 48),
                        ACCENT_GREEN,
                    ),
                    MapState::SetupRequired => (
                        "Map: Scan Required",
                        Color32::from_rgb(72, 54, 16),
                        ACCENT_AMBER,
                    ),
                    MapState::NoTrackerConnected => (
                        "No Tracker Connected",
                        BG_SUBTLE,
                        TEXT_MUTED,
                    ),
                    MapState::Unknown(_) => (
                        "Map: Checking",
                        BG_SUBTLE,
                        TEXT_MUTED,
                    ),
                },
            };
            pill(ui, map_lbl, map_bg, map_fg);

            if snap.server_state == ServerState::Running && snap.tracker_count > 0 {
                pill(
                    ui,
                    &format!("Dongle: {} tracker(s)", snap.tracker_count),
                    Color32::from_rgb(20, 56, 78),
                    ACCENT_CYAN,
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if scanning_or_busy {
                    let cancel_btn = egui::Button::new(
                        RichText::new("Cancel Scan").strong().color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(110, 38, 46));
                    if ui.add(cancel_btn).clicked() {
                        engine.scanner.cancel_scan();
                    }
                    if matches!(snap.phase, ScanPhase::Scanning { .. }) {
                        let fin_btn = egui::Button::new(
                            RichText::new("Finish & Save Map").strong().color(Color32::BLACK),
                        )
                        .fill(ACCENT_GREEN);
                        if ui.add(fin_btn).clicked() {
                            engine.scanner.finish_scan();
                        }
                    }
                } else {
                    let needs_scan = snap.map_state == MapState::SetupRequired;
                    let label = if needs_scan {
                        "Scan Room Now"
                    } else {
                        "Scan Room"
                    };
                    let scan_btn = egui::Button::new(
                        RichText::new(label)
                            .strong()
                            .color(if needs_scan { Color32::BLACK } else { Color32::WHITE }),
                    )
                    .fill(if needs_scan {
                        ACCENT_CYAN
                    } else {
                        Color32::from_rgb(24, 78, 108)
                    });
                    if ui
                        .add_enabled(snap.server_state != ServerState::NotInstalled, scan_btn)
                        .on_hover_text(
                            "Start a room scan directly via ViveTrackerServer (no HTC app or headset needed)",
                        )
                        .clicked()
                    {
                        engine.scanner.start_scan();
                    }

                    let pair_btn = egui::Button::new(
                        RichText::new(if snap.pairing_busy {
                            "Pairing..."
                        } else {
                            "Pair Tracker"
                        })
                        .strong()
                        .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(44, 58, 86));
                    if ui
                        .add_enabled(
                            snap.server_state != ServerState::NotInstalled && !snap.pairing_busy,
                            pair_btn,
                        )
                        .on_hover_text(
                            "Auto-picks the next free dongle slot (1–5) and starts pairing mode. Hold tracker power button until LED blinks blue.",
                        )
                        .clicked()
                    {
                        engine.scanner.pair_tracker(None);
                    }

                    if snap.server_state == ServerState::Offline {
                        if ui.button("Start Tracker Server").clicked() {
                            engine.scanner.launch_server();
                        }
                    }
                }
            });
        });

        ui.add_space(6.0);

        match &snap.phase {
            ScanPhase::Idle => {
                let hint = match snap.server_state {
                    ServerState::NotInstalled => {
                        "Install VIVE Hub runtime so ViveTrackerServer.exe is available."
                    }
                    ServerState::Offline => {
                        "ViveTrackerServer.exe is offline. Click 'Start Tracker Server' or 'Scan Room' to launch it silently."
                    }
                    ServerState::Launching | ServerState::Checking => {
                        "Launching ViveTrackerServer.exe silently in standalone mode (no VIVE Hub UI)..."
                    }
                    ServerState::Running => match snap.map_state {
                        MapState::SetupRequired => {
                            "No room map on trackers yet. Click 'Scan Room Now', then slowly pan/turn 1 tracker 360° around your room — audio cues guide you hands-free."
                        }
                        MapState::NoTrackerConnected => {
                            "Standalone tracker server is running. Turn on at least 1 VIVE Ultimate Tracker to scan or track."
                        }
                        MapState::Ready => {
                            "Room map is active on your trackers (no HTC app needed). Click 'Scan Room' anytime if lighting or furniture changed."
                        }
                        MapState::Unknown(_) => "Connected to ViveTrackerServer.",
                    },
                };
                ui.label(RichText::new(hint).size(12.0).color(TEXT_MUTED));
            }
            ScanPhase::Starting => {
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 38, 52))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0_f32, ACCENT_CYAN.linear_multiply(0.5)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("Initializing room scan on tracker...")
                                .strong()
                                .color(ACCENT_CYAN),
                        );
                    });
            }
            ScanPhase::Scanning {
                elapsed_s,
                host_serial,
                coverage,
                overall_pct,
                low_light,
                overspeed,
            } => {
                let target = engine.scanner.target_quality;
                egui::Frame::new()
                    .fill(Color32::from_rgb(16, 42, 58))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0_f32, ACCENT_CYAN.linear_multiply(0.6)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            draw_coverage_ring(ui, *coverage, *overall_pct, target);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "SCANNING ROOM — {overall_pct}%  ({elapsed_s:.1}s)"
                                    ))
                                    .strong()
                                    .size(15.0)
                                    .color(if *overall_pct >= target {
                                        ACCENT_GREEN
                                    } else {
                                        ACCENT_CYAN
                                    }),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "Host tracker: {host_serial}  •  Slowly pan 360° around your playspace (low & high walls)."
                                    ))
                                    .size(12.0)
                                    .color(Color32::from_rgb(220, 228, 240)),
                                );
                                ui.add_space(4.0);
                                ui.add(
                                    egui::ProgressBar::new((*overall_pct as f32 / 100.0).clamp(0.0, 1.0))
                                        .text(format!(
                                            "Coverage: {overall_pct}% / Target {target}%"
                                        ))
                                        .fill(if *overall_pct >= target {
                                            ACCENT_GREEN
                                        } else {
                                            ACCENT_CYAN
                                        })
                                        .desired_height(16.0),
                                );
                                if *overspeed || *low_light {
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        if *overspeed {
                                            pill(
                                                ui,
                                                "⚠ Moving too fast — slow down",
                                                Color32::from_rgb(76, 26, 30),
                                                ACCENT_RED,
                                            );
                                        }
                                        if *low_light {
                                            pill(
                                                ui,
                                                "⚠ Low light — brighten room",
                                                Color32::from_rgb(72, 54, 16),
                                                ACCENT_AMBER,
                                            );
                                        }
                                    });
                                }
                            });
                        });
                    });
            }
            ScanPhase::Finalizing { elapsed_s, note } => {
                egui::Frame::new()
                    .fill(Color32::from_rgb(16, 54, 52))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 10))
                    .stroke(Stroke::new(1.0_f32, ACCENT_GREEN.linear_multiply(0.5)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(format!("Saving & Syncing Map... ({elapsed_s:.1}s)"))
                                .strong()
                                .size(14.5)
                                .color(ACCENT_GREEN),
                        );
                        ui.label(
                            RichText::new(note)
                                .size(12.0)
                                .color(Color32::from_rgb(220, 228, 240)),
                        );
                    });
            }
            ScanPhase::Completed { message } => {
                egui::Frame::new()
                    .fill(Color32::from_rgb(16, 62, 48))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0_f32, ACCENT_GREEN.linear_multiply(0.5)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(message).strong().size(12.5).color(ACCENT_GREEN));
                    });
            }
            ScanPhase::Error { message } => {
                egui::Frame::new()
                    .fill(Color32::from_rgb(76, 26, 30))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0_f32, ACCENT_RED.linear_multiply(0.5)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(message).size(12.5).color(ACCENT_RED));
                    });
            }
        }

        if let Some(note) = &snap.pair_status {
            ui.add_space(5.0);
            ui.label(RichText::new(format!("Pairing: {note}")).size(12.0).color(ACCENT_CYAN));
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut engine.scanner.auto_finish, "Auto-Finish at Target");
            ui.add_space(8.0);
            ui.label(RichText::new("Target Quality:").size(12.0).color(TEXT_MUTED));
            let mut tq = engine.scanner.target_quality as i32;
            if ui
                .add(egui::Slider::new(&mut tq, 50..=90).suffix("%"))
                .changed()
            {
                engine.scanner.target_quality = tq as u8;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_pair =
                    snap.server_state != ServerState::NotInstalled && !snap.pairing_busy && !scanning_or_busy;
                if ui
                    .add_enabled(can_pair, egui::Button::new("Unpair").small())
                    .on_hover_text("Unpair the selected dongle slot (1–5)")
                    .clicked()
                {
                    engine.scanner.unpair_slot(engine.scanner.selected_slot);
                }
                if ui
                    .add_enabled(can_pair, egui::Button::new("Pair Slot").small())
                    .on_hover_text("Pair a tracker into the specific dongle slot (1–5)")
                    .clicked()
                {
                    engine.scanner.pair_tracker(Some(engine.scanner.selected_slot));
                }
                for slot in (0..5u32).rev() {
                    let selected = engine.scanner.selected_slot == slot;
                    let occupied = snap
                        .paired_trackers
                        .iter()
                        .any(|t| t.display_num == slot + 1);
                    let btn = egui::Button::new(
                        RichText::new(format!("{}", slot + 1))
                            .size(11.0)
                            .strong()
                            .color(if selected {
                                Color32::BLACK
                            } else if occupied {
                                ACCENT_GREEN
                            } else {
                                TEXT_MUTED
                            }),
                    )
                    .fill(if selected { ACCENT_CYAN } else { BG_SUBTLE })
                    .small();
                    if ui
                        .add(btn)
                        .on_hover_text(if occupied {
                            format!("Dongle Slot #{} (Paired)", slot + 1)
                        } else {
                            format!("Dongle Slot #{} (Free)", slot + 1)
                        })
                        .clicked()
                    {
                        engine.scanner.selected_slot = slot;
                    }
                }
                ui.label(RichText::new("Dongle Slot:").size(12.0).color(TEXT_MUTED));
            });
        });
    });
}

fn draw_coverage_ring(ui: &mut egui::Ui, coverage: [u8; 8], overall_pct: u8, target: u8) {
    let size = Vec2::splat(76.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = 30.0_f32;
    let step = std::f32::consts::TAU / 8.0;

    for i in 0..8 {
        let pct = scanner::normalize_sector_pct(coverage, i);
        let col = if pct >= target {
            ACCENT_GREEN
        } else if pct >= 40 {
            ACCENT_AMBER
        } else if pct > 0 {
            ACCENT_CYAN
        } else {
            Color32::from_rgb(48, 56, 72)
        };
        let a0 = -std::f32::consts::FRAC_PI_2 + (i as f32) * step + 0.08;
        let a1 = a0 + step - 0.16;
        let segs = 6;
        let mut pts = Vec::with_capacity(segs + 1);
        for s in 0..=segs {
            let a = a0 + (a1 - a0) * (s as f32 / segs as f32);
            pts.push(Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin()));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(6.0_f32, col)));
    }

    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        format!("{overall_pct}%"),
        egui::FontId::proportional(15.0),
        if overall_pct >= target {
            ACCENT_GREEN
        } else {
            Color32::WHITE
        },
    );
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

fn draw_sleeve_card(ui: &mut egui::Ui, engine: &mut CalibrationEngine) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Hand Sleeves & Switch Joy-Cons")
                    .strong()
                    .size(14.0)
                    .color(Color32::WHITE),
            );
            let (bg, fg, lbl) = if engine.sleeve.streaming {
                (Color32::from_rgb(16, 62, 48), ACCENT_GREEN, "VR Bridge: Streaming")
            } else if engine.sleeve.ui_running {
                (Color32::from_rgb(20, 56, 78), ACCENT_CYAN, "UI Running (Paused)")
            } else {
                (BG_SUBTLE, TEXT_MUTED, "Bridge: Idle")
            };
            pill(ui, lbl, bg, fg);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open Full Button/Offset UI").clicked() {
                    match engine.sleeve.open_sleeve_ui() {
                        Ok(msg) => engine.log(LogLevel::Accent, format!("[Sleeve] {msg}")),
                        Err(e) => engine.log(LogLevel::Error, format!("[Sleeve] {e}")),
                    }
                }
                let next_stream = !engine.sleeve.streaming;
                let stream_btn = egui::Button::new(
                    RichText::new(if next_stream {
                        "Start VR Hand Bridge"
                    } else {
                        "Stop VR Hand Bridge"
                    })
                    .strong()
                    .color(if next_stream { Color32::BLACK } else { Color32::WHITE }),
                )
                .fill(if next_stream {
                    ACCENT_GREEN
                } else {
                    Color32::from_rgb(110, 38, 46)
                });
                if ui.add(stream_btn).clicked() {
                    match engine.sleeve.set_streaming(next_stream) {
                        Ok(msg) => engine.log(LogLevel::Success, format!("[Sleeve] {msg}")),
                        Err(e) => engine.log(LogLevel::Error, format!("[Sleeve] {e}")),
                    }
                }
            });
        });

        ui.separator();

        ui.columns(2, |cols| {
            let l_ser = engine
                .sleeve
                .left_serial
                .as_deref()
                .unwrap_or("Auto (mark [L] above)");
            cols[0].horizontal(|ui| {
                ui.label(RichText::new("Left Sleeve:").color(ACCENT_CYAN).strong().size(12.5));
                ui.monospace(l_ser);
            });
            cols[0].horizontal(|ui| {
                ui.label(
                    RichText::new(format!("L1={}  •  L2={}", engine.sleeve.l1, engine.sleeve.l2))
                        .size(12.0)
                        .color(TEXT_MUTED),
                );
                if ui.small_button("Swap L1 ↔ L2").clicked() {
                    match engine.sleeve.swap_shoulders(true) {
                        Ok(msg) => engine.log(LogLevel::Info, format!("[Sleeve] {msg}")),
                        Err(e) => engine.log(LogLevel::Error, format!("[Sleeve] {e}")),
                    }
                }
            });

            let r_ser = engine
                .sleeve
                .right_serial
                .as_deref()
                .unwrap_or("Auto (mark [R] above)");
            cols[1].horizontal(|ui| {
                ui.label(
                    RichText::new("Right Sleeve:")
                        .color(Color32::from_rgb(251, 113, 133))
                        .strong()
                        .size(12.5),
                );
                ui.monospace(r_ser);
            });
            cols[1].horizontal(|ui| {
                ui.label(
                    RichText::new(format!("R1={}  •  R2={}", engine.sleeve.r1, engine.sleeve.r2))
                        .size(12.0)
                        .color(TEXT_MUTED),
                );
                if ui.small_button("Swap R1 ↔ R2").clicked() {
                    match engine.sleeve.swap_shoulders(false) {
                        Ok(msg) => engine.log(LogLevel::Info, format!("[Sleeve] {msg}")),
                        Err(e) => engine.log(LogLevel::Error, format!("[Sleeve] {e}")),
                    }
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
