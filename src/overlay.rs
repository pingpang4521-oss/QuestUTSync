use crate::openvr::{
    OverlayApi, VrTexture, VrTextureBounds, COLOR_SPACE_GAMMA, TEXTURE_TYPE_OPENGL,
    VR_EVENT_MOUSE_BUTTON_DOWN, VR_EVENT_MOUSE_BUTTON_UP, VR_EVENT_MOUSE_MOVE,
    VR_EVENT_SCROLL_DISCRETE, VR_EVENT_SCROLL_SMOOTH, VR_MOUSE_BUTTON_LEFT, VR_MOUSE_BUTTON_MIDDLE,
    VR_MOUSE_BUTTON_RIGHT, VR_OVERLAY_FLAG_SEND_DISCRETE_SCROLL,
    VR_OVERLAY_FLAG_SEND_SMOOTH_SCROLL, VR_OVERLAY_INPUT_MOUSE,
};
use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

pub const OVERLAY_KEY: &str = "questutsync.dashboard";
pub const OVERLAY_NAME: &str = "QuestUTSync";
const OVERLAY_W: u32 = 960;
const OVERLAY_H: u32 = 680;

pub struct DashboardOverlay {
    api: OverlayApi,
    main_handle: u64,
    thumb_handle: u64,
    gl: Arc<glow::Context>,
    fbo: glow::Framebuffer,
    color_tex: glow::Texture,
    painter: egui_glow::Painter,
    ctx: egui::Context,
    start_time: Instant,
    last_pointer: egui::Pos2,
    rendered_once: bool,
}

impl DashboardOverlay {
    pub fn try_create(api: OverlayApi, gl: Arc<glow::Context>) -> Result<Self, String> {
        let (main_handle, thumb_handle) = api
            .create_dashboard_overlay(OVERLAY_KEY, OVERLAY_NAME)
            .map_err(|e| format!("CreateDashboardOverlay failed (EVROverlayError={e})"))?;

        api.set_width_in_meters(main_handle, 2.2);
        api.set_input_method(main_handle, VR_OVERLAY_INPUT_MOUSE);
        api.set_flag(main_handle, VR_OVERLAY_FLAG_SEND_DISCRETE_SCROLL, true);
        api.set_flag(main_handle, VR_OVERLAY_FLAG_SEND_SMOOTH_SCROLL, true);
        api.set_mouse_scale(main_handle, OVERLAY_W as f32, OVERLAY_H as f32);
        // OpenGL FBO origin is bottom-left; OpenVR expects UV min at upper-left, UV max at lower-right.
        api.set_texture_bounds(
            main_handle,
            VrTextureBounds {
                u_min: 0.0,
                v_min: 1.0,
                u_max: 1.0,
                v_max: 0.0,
            },
        );

        let icon = generate_dashboard_icon(128);
        api.set_raw_rgba(thumb_handle, &icon, 128, 128);

        let (fbo, color_tex) = unsafe { create_offscreen_target(&gl, OVERLAY_W, OVERLAY_H)? };
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false)
            .map_err(|e| format!("egui_glow::Painter init failed: {e}"))?;

        let ctx = egui::Context::default();
        crate::ui::apply_style(&ctx);

        Ok(Self {
            api,
            main_handle,
            thumb_handle,
            gl,
            fbo,
            color_tex,
            painter,
            ctx,
            start_time: Instant::now(),
            last_pointer: egui::Pos2::ZERO,
            rendered_once: false,
        })
    }

    pub fn is_visible(&self) -> bool {
        self.api.is_visible(self.main_handle)
    }

    pub fn show_in_dashboard(&self) {
        self.api.show_dashboard(OVERLAY_KEY);
    }

    /// Polls overlay pointer events and renders the shared UI into the OpenVR dashboard texture.
    /// Returns `true` if the overlay is currently visible (so the caller can request higher repaint rate).
    pub fn render_if_visible(&mut self, mut draw_ui: impl FnMut(&egui::Context)) -> bool {
        let visible = self.is_visible();
        if !visible && self.rendered_once {
            return false;
        }

        let mut events = Vec::new();
        while let Some(ev) = self.api.poll_event(self.main_handle) {
            match ev.event_type {
                VR_EVENT_MOUSE_MOVE => {
                    let m = unsafe { ev.data.mouse };
                    // VREvent_Mouse_t is in GL coords (bottom-left = 0,0); egui uses top-left = 0,0.
                    let pos = egui::pos2(m.x, OVERLAY_H as f32 - m.y);
                    self.last_pointer = pos;
                    events.push(egui::Event::PointerMoved(pos));
                }
                VR_EVENT_MOUSE_BUTTON_DOWN | VR_EVENT_MOUSE_BUTTON_UP => {
                    let m = unsafe { ev.data.mouse };
                    let pos = egui::pos2(m.x, OVERLAY_H as f32 - m.y);
                    self.last_pointer = pos;
                    if let Some(button) = map_vr_button(m.button) {
                        events.push(egui::Event::PointerButton {
                            pos,
                            button,
                            pressed: ev.event_type == VR_EVENT_MOUSE_BUTTON_DOWN,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                }
                VR_EVENT_SCROLL_DISCRETE | VR_EVENT_SCROLL_SMOOTH => {
                    let s = unsafe { ev.data.scroll };
                    let scale = if ev.event_type == VR_EVENT_SCROLL_SMOOTH {
                        40.0
                    } else {
                        24.0
                    };
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(s.xdelta * scale, s.ydelta * scale),
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                _ => {}
            }
        }

        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(OVERLAY_W as f32, OVERLAY_H as f32),
            )),
            time: Some(self.start_time.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };

        let full_output = self.ctx.run(raw_input, |ctx| draw_ui(ctx));
        let primitives = self.ctx.tessellate(full_output.shapes, 1.0);

        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl
                .viewport(0, 0, OVERLAY_W as i32, OVERLAY_H as i32);
            self.gl.clear_color(0.08, 0.09, 0.11, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);

            self.painter.paint_and_update_textures(
                [OVERLAY_W, OVERLAY_H],
                1.0,
                &primitives,
                &full_output.textures_delta,
            );

            self.gl.flush();
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }

        let tex = VrTexture {
            handle: self.color_tex.0.get() as usize as *mut c_void,
            texture_type: TEXTURE_TYPE_OPENGL,
            color_space: COLOR_SPACE_GAMMA,
        };
        self.api.set_texture(self.main_handle, tex);
        self.rendered_once = true;
        visible
    }
}

impl Drop for DashboardOverlay {
    fn drop(&mut self) {
        self.api.destroy_overlay(self.main_handle);
        self.api.destroy_overlay(self.thumb_handle);
        self.painter.destroy();
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.color_tex);
        }
    }
}

fn map_vr_button(btn: u32) -> Option<egui::PointerButton> {
    match btn {
        VR_MOUSE_BUTTON_LEFT => Some(egui::PointerButton::Primary),
        VR_MOUSE_BUTTON_RIGHT => Some(egui::PointerButton::Secondary),
        VR_MOUSE_BUTTON_MIDDLE => Some(egui::PointerButton::Middle),
        _ => None,
    }
}

unsafe fn create_offscreen_target(
    gl: &glow::Context,
    w: u32,
    h: u32,
) -> Result<(glow::Framebuffer, glow::Texture), String> {
    let tex = gl.create_texture().map_err(|e| format!("create_texture: {e}"))?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        w as i32,
        h as i32,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(None),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);

    let fbo = gl
        .create_framebuffer()
        .map_err(|e| format!("create_framebuffer: {e}"))?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(tex),
        0,
    );
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);

    if status != glow::FRAMEBUFFER_COMPLETE {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(tex);
        return Err(format!("Incomplete overlay FBO (status=0x{status:x})"));
    }

    Ok((fbo, tex))
}

/// Generates a crisp 128x128 RGBA dashboard tab icon (dark rounded square + cyan calibration rings & 3 tracker dots).
fn generate_dashboard_icon(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let c = s * 0.5;
    let dots = [
        (c, c - s * 0.14),         // waist
        (c - s * 0.14, c + s * 0.15), // left foot
        (c + s * 0.14, c + s * 0.15), // right foot
    ];

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = (fx - c).abs();
            let dy = (fy - c).abs();

            // Rounded tile mask
            let r_corner = s * 0.18;
            let half = s * 0.46;
            let qx = (dx - (half - r_corner)).max(0.0);
            let qy = (dy - (half - r_corner)).max(0.0);
            let sdf_box = (qx * qx + qy * qy).sqrt() - r_corner;
            let mut px = [0u8, 0, 0, 0];

            if sdf_box <= 0.0 {
                px = [22, 27, 36, 255];
                if sdf_box > -3.0 {
                    px = [56, 189, 248, 255];
                }

                let dist = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
                if (dist - s * 0.30).abs() <= 2.6 {
                    px = [56, 189, 248, 255];
                }
                for (i, &(tx, ty)) in dots.iter().enumerate() {
                    let d = ((fx - tx).powi(2) + (fy - ty).powi(2)).sqrt();
                    if d <= s * 0.065 {
                        px = if i == 1 {
                            [52, 211, 153, 255] // left foot highlighted emerald
                        } else {
                            [226, 232, 240, 255]
                        };
                    }
                }
            }

            let idx = ((y * size + x) * 4) as usize;
            out[idx..idx + 4].copy_from_slice(&px);
        }
    }
    out
}
