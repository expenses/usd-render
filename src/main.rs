use bbl_usd::usd;
use dolly::prelude::*;
use glfw::{Action, Context, Key};
use glow::HasContext;
use iroh_net::ticket::NodeTicket;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

mod networking;

const ALPN: &[u8] = b"myalpn";

struct Log {
    inner: Arc<parking_lot::RwLock<Vec<String>>>,
}

impl Log {
    fn new() -> (Box<Self>, Arc<parking_lot::RwLock<Vec<String>>>) {
        log::set_max_level(log::LevelFilter::Info);

        let lines: Arc<parking_lot::RwLock<Vec<String>>> = Default::default();

        (
            Box::new(Self {
                inner: lines.clone(),
            }),
            lines,
        )
    }
}

impl log::Log for Log {
    fn flush(&self) {}

    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target() == "usd_render"
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let mut inner = self.inner.write();

        inner.push(format!("[{}] {}", record.level(), record.args()));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (logger, log_lines) = Log::new();
    log::set_boxed_logger(logger)?;

    let connected_nodes = networking::ConnectedNodes::default();

    let endpoint_handle = tokio::spawn(
        iroh_net::MagicEndpoint::builder()
            .alpns(vec![ALPN.to_owned()])
            .bind(0),
    );

    let mut glfw_backend =
        egui_window_glfw_passthrough::GlfwBackend::new(egui_window_glfw_passthrough::GlfwConfig {
            ..Default::default()
        });

    glfw_backend.window.make_current();
    glfw_backend.window.set_key_polling(true);

    let gl = unsafe {
        glow::Context::from_loader_function(|s| glfw_backend.window.get_proc_address(s) as *const _)
    };

    #[allow(clippy::arc_with_non_send_sync)]
    let gl = Arc::new(gl);

    let egui = egui::Context::default();

    let mut painter = egui_glow::painter::Painter::new(gl.clone(), "", None)?;

    let engine = usd::GLEngine::new();

    let stage = usd::Stage::open(std::env::args().nth(1).unwrap()).unwrap();

    let prim = stage.pseudo_root();

    let mut camera: CameraRig = CameraRig::builder()
        .with(Position::new(glam::Vec3::splat(2.0)))
        .with(YawPitch {
            yaw_degrees: 45.0,
            pitch_degrees: -45.0,
        })
        .with(Smooth::new_position_rotation(1.0, 0.1))
        .build();

    unsafe {
        gl.clear_color(0.1, 0.2, 0.3, 1.0);
    }

    let params = usd::GLRenderParams::new();

    let mut size = glfw_backend.window.get_size();
    engine.set_render_viewport(glam::DVec4::new(0.0, 0.0, size.0 as _, size.1 as _));

    let mut grab_toggled = false;
    let mut prev_cursor_pos = glam::DVec2::from(glfw_backend.window.get_cursor_pos());

    let proj = glam::DMat4::perspective_rh_gl(59.0_f64.to_radians(), 1.0, 0.01, 1000.0);

    let mut text = String::new();

    let endpoint = endpoint_handle.await??;

    tokio::spawn({
        let endpoint = endpoint.clone();
        let connected_nodes = connected_nodes.clone();
        async move {
            while let Some(connecting) = endpoint.accept().await {
                tokio::spawn(networking::accept(connecting, connected_nodes.clone()));
            }
        }
    });

    let addr = endpoint.my_addr().await?;
    let ticket = iroh_net::ticket::NodeTicket::new(addr.clone())?;

    println!("{}", ticket);

    while !glfw_backend.window.should_close() {
        glfw_backend.glfw.poll_events();
        glfw_backend.tick();

        egui.begin_frame(glfw_backend.take_raw_input());

        // Handle key presses
        if !egui.wants_keyboard_input() {
            for event in glfw_backend.frame_events.iter() {
                match event {
                    glfw::WindowEvent::Key(Key::Escape, _, Action::Press, _) => {
                        glfw_backend.window.set_should_close(true)
                    }
                    glfw::WindowEvent::Key(Key::G, _, Action::Press, _) => {
                        grab_toggled = !grab_toggled;
                        if grab_toggled {
                            glfw_backend
                                .window
                                .set_cursor_mode(glfw::CursorMode::Disabled);
                        } else {
                            glfw_backend
                                .window
                                .set_cursor_mode(glfw::CursorMode::Normal);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Handle resizing
        let current_size = glfw_backend.window.get_size();
        if current_size != size {
            size = current_size;
            engine.set_render_viewport(glam::DVec4::new(0.0, 0.0, size.0 as _, size.1 as _));
        }

        // Get mouse delta
        let cursor_pos = glam::DVec2::from(glfw_backend.window.get_cursor_pos());
        let delta = cursor_pos - prev_cursor_pos;
        prev_cursor_pos = cursor_pos;

        // Camera movement and rotation

        let movement = if egui.wants_keyboard_input() {
            glam::IVec3::ZERO
        } else {
            get_movement(&glfw_backend.window)
        }
        .as_vec3()
            * 0.1;

        let movement = movement.x * camera.final_transform.right()
            + movement.y * camera.final_transform.forward()
            + movement.z * glam::Vec3::Y;

        camera.driver_mut::<Position>().translate(movement);

        if grab_toggled {
            let yaw_pitch = -delta * 0.25;
            camera
                .driver_mut::<YawPitch>()
                .rotate_yaw_pitch(yaw_pitch.x as _, yaw_pitch.y as _);
        }

        // Update usd camera state

        let transform = camera.update(1.0 / 60.0);

        engine.set_camera_state(view_from_camera_transform(transform), proj);

        // Egui

        {
            let connection_infos = endpoint.connection_infos().await?;
            egui::Window::new("Network").show(&egui, |ui| {
                ui.label("Node Ticket (click to copy):");
                let node_ticket_str = ticket.to_string();
                if ui.button(&node_ticket_str).clicked() {
                    glfw_backend.window.set_clipboard_string(&node_ticket_str);
                }

                let response = ui
                    .horizontal(|ui| {
                        ui.label("Connect to node: ");
                        ui.add(egui::widgets::text_edit::TextEdit::singleline(&mut text))
                    })
                    .inner;

                if response.lost_focus()
                    && response.ctx.input(|ctx| ctx.key_pressed(egui::Key::Enter))
                {
                    match NodeTicket::from_str(&text) {
                        Ok(ticket) => {
                            if *ticket.node_addr() == addr {
                                log::error!("Refusing to connect to self");
                            } else {
                                tokio::spawn(networking::connect(
                                    endpoint.clone(),
                                    ticket.node_addr().clone(),
                                    connected_nodes.clone(),
                                ));
                                text.clear();
                            }
                        }
                        Err(error) => {
                            log::error!("bad ticket {}: {}", text, error);
                        }
                    }
                }

                ui.heading("Connections");

                draw_connection_grid(ui, &connection_infos);

                let log_lines = log_lines.read();

                for line in log_lines.iter() {
                    ui.label(line);
                }
            });
        }

        let output = egui.end_frame();
        let meshes = egui.tessellate(output.shapes, output.pixels_per_point);

        // Rendering

        unsafe {
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }

        engine.render(&prim, &params);

        painter.paint_and_update_textures(
            [size.0 as u32, size.1 as u32],
            output.pixels_per_point,
            &meshes,
            &output.textures_delta,
        );

        glfw_backend.window.swap_buffers();
    }

    Ok(())
}

fn get_movement(window: &glfw::Window) -> glam::IVec3 {
    let mut movement = glam::IVec3::ZERO;

    movement.x += (window.get_key(glfw::Key::D) != glfw::Action::Release) as i32;
    movement.x -= (window.get_key(glfw::Key::A) != glfw::Action::Release) as i32;

    movement.y += (window.get_key(glfw::Key::W) != glfw::Action::Release) as i32;
    movement.y -= (window.get_key(glfw::Key::S) != glfw::Action::Release) as i32;

    movement.z += (window.get_key(glfw::Key::Q) != glfw::Action::Release) as i32;
    movement.z -= (window.get_key(glfw::Key::Z) != glfw::Action::Release) as i32;

    movement
}

fn view_from_camera_transform(
    transform: dolly::transform::Transform<dolly::handedness::RightHanded>,
) -> glam::DMat4 {
    glam::DMat4::look_at_rh(
        transform.position.as_dvec3(),
        transform.position.as_dvec3() + transform.forward().as_dvec3(),
        transform.up().as_dvec3(),
    )
}

fn draw_connection(ui: &mut egui::Ui, connection_info: &iroh_net::magicsock::EndpointInfo) {
    ui.label(connection_info.id.to_string());
    ui.label(connection_info.public_key.fmt_short());
    ui.label(format!("{}", connection_info.conn_type));
    ui.label(match connection_info.latency {
        Some(duration) => format!("{:.2} ms", duration.as_secs_f32() * 1000.0),
        None => "N/A".to_string(),
    });
    ui.label(match connection_info.last_used {
        Some(duration) => format!("{:.2} s", duration.as_secs_f32()),
        None => "Never".to_string(),
    });
}

fn draw_connection_grid(ui: &mut egui::Ui, connection_infos: &[iroh_net::magicsock::EndpointInfo]) {
    if connection_infos.is_empty() {
        ui.label("No current connections");
    } else {
        egui::Grid::new("connection_grid")
            .striped(true)
            .show(ui, |ui| {
                for connection_info in connection_infos {
                    draw_connection(ui, connection_info);
                    ui.end_row();
                }
            });
    }
}
