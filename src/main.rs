use bbl_usd::{cpp, sdf, usd};
use dolly::prelude::*;
use glfw::{Action, Context, Key};
use glow::HasContext;
use iroh_net::ticket::NodeTicket;
use std::str::FromStr;
use std::sync::Arc;

mod logging;
mod networking;

use networking::{NodeApprovalResponse, NodeSharingPolicy};

const ALPN: &[u8] = b"myalpn";

fn compare_and_send<T: PartialEq>(sender: &mut tokio::sync::watch::Sender<T>, value: T) {
    sender.send_if_modified(|current| {
        if *current == value {
            return false;
        }
        *current = value;
        true
    });
}

fn export_layer_to_string(layer: &sdf::LayerRefPtr) -> String {
    let string = layer.export_to_string().unwrap();
    string.as_str().to_owned()
}

struct LocalLayers {
    root: sdf::LayerRefPtr,
    current_sublayer: sdf::LayerRefPtr,
    private: sdf::LayerRefPtr,
    sublayer_index: u8,
}

impl LocalLayers {
    fn new(root: &sdf::LayerHandle) -> Self {
        let local_root = sdf::Layer::create_anonymous(".usdc");

        root.insert_sub_layer_path(local_root.get_identifier(), 0);

        let current_sublayer = sdf::Layer::create_anonymous(".usdc");

        local_root.insert_sub_layer_path(current_sublayer.get_identifier(), 0);

        let private = sdf::Layer::create_anonymous(".usdc");

        root.insert_sub_layer_path(private.get_identifier(), 0);

        Self {
            root: local_root,
            current_sublayer,
            private,
            sublayer_index: 0,
        }
    }

    fn set_private_edit_target(&mut self, stage: &usd::StageRefPtr) {
        let edit_target = usd::EditTarget::new_from_layer_ref_ptr(&self.private);
        stage.set_edit_target(&edit_target);
    }

    fn set_public_edit_target(&mut self, stage: &usd::StageRefPtr) {
        let edit_target = usd::EditTarget::new_from_layer_ref_ptr(&self.current_sublayer);
        stage.set_edit_target(&edit_target);
    }

    fn add_new_sublayer(&mut self) {
        let new_sublayer = sdf::Layer::create_anonymous(".usdc");

        self.root
            .insert_sub_layer_path(new_sublayer.get_identifier(), 0);

        self.current_sublayer = new_sublayer;
        self.sublayer_index += 1;
    }

    fn export(&self) -> (u8, cpp::String) {
        let state = self.current_sublayer.export_to_string().unwrap();
        (self.sublayer_index, state)
    }
}

struct UsdState {
    stage: usd::StageRefPtr,
    root_layer: sdf::LayerHandle,
    pseudo_root: usd::Prim,
    xformable_avatar: usd::XformCommonAPI,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::setup()?;

    let approved_nodes = networking::ApprovedNodes::default();
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::channel(10);
    let connected_nodes = networking::ConnectedNodes::default();

    let endpoint = iroh_net::MagicEndpoint::builder()
        .alpns(vec![ALPN.to_owned()])
        .bind(0)
        .await?;

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

    let stage = usd::Stage::create_in_memory();
    // Root layer that holds all other layers.
    let root_layer = stage.get_root_layer();

    let base_layer = sdf::Layer::find_or_open(&std::env::args().nth(1).unwrap());

    root_layer.insert_sub_layer_path(base_layer.get_identifier(), 0);

    let mut local_layers = LocalLayers::new(&root_layer);

    let prim = stage.pseudo_root();

    local_layers.set_public_edit_target(&stage);

    // note: prefix with _avatar as names can't start with numbers.
    let avatar = stage
        .define_prim(
            &format!("/avatars/avatar_{}", endpoint.node_id().fmt_short())[..],
            "Sphere",
        )
        .map_err(|err| anyhow::anyhow!("{:?}", err))?;

    local_layers.set_private_edit_target(&stage);

    {
        let xformable_avatar = usd::XformCommonAPI::new(&avatar);
        xformable_avatar.set_scale(glam::Vec3::splat(0.0), Default::default());
    }

    let (mut state_tx, state_rx) = tokio::sync::watch::channel((0_u8, cpp::String::default()));

    println!("{}", export_layer_to_string(&local_layers.private));

    compare_and_send(&mut state_tx, local_layers.export());

    local_layers.add_new_sublayer();

    local_layers.set_public_edit_target(&stage);

    let xformable_avatar = usd::XformCommonAPI::new(&avatar);

    let usd_state = Arc::new(tokio::sync::RwLock::new(UsdState {
        stage,
        root_layer,
        pseudo_root: prim,
        xformable_avatar,
    }));

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

    let addr = endpoint.my_addr().await?;

    let networking_state = networking::State {
        endpoint: endpoint.clone(),
        approved_nodes: approved_nodes.clone(),
        approval_queue: approval_tx.clone(),
        connected_nodes: connected_nodes.clone(),
        state: state_rx.clone(),
        usd: usd_state.clone(),
        exported_local_layers: Default::default(),
    };

    tokio::spawn(networking::update_exported_local_layers(
        networking_state.clone(),
    ));

    tokio::spawn({
        let networking_state = networking_state.clone();
        async move {
            while let Some(connecting) = networking_state.endpoint.accept().await {
                tokio::spawn(networking::accept(connecting, networking_state.clone()));
            }
        }
    });

    let ticket = iroh_net::ticket::NodeTicket::new(addr.clone())?;

    println!("{}", ticket);

    let mut approval_queue = Vec::new();

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

        // Egui

        {
            let connection_infos = endpoint.connection_infos().await?;

            while let Ok(request) = approval_rx.try_recv() {
                approval_queue.push((
                    request.node_id,
                    request.direction,
                    Some(request.response_sender),
                ));
            }

            let approved_nodes_read = approved_nodes.read().await;

            let log_lines = logging::get_lines().await;

            egui::Window::new("Network").show(&egui, |ui| {
                ui.label(format!("Node ID: {}", addr.node_id.fmt_short()));
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
                            let node_addr = ticket.node_addr().clone();
                            if node_addr == addr {
                                log::error!("Not connecting to self.");
                            } else {
                                tokio::spawn(networking::connect(
                                    networking_state.clone(),
                                    node_addr,
                                    None,
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

                if !approval_queue.is_empty() {
                    ui.heading("Approval Queue");

                    approval_queue.retain_mut(|(node_id, direction, sender)| {
                        if sender.is_none() {
                            return false;
                        }

                        if let Some(node_sharing) = approved_nodes_read.get(node_id) {
                            let _ = sender
                                .take()
                                .unwrap()
                                .send(NodeApprovalResponse::Approved(node_sharing.clone()));
                            return false;
                        }

                        ui.horizontal(|ui| {
                            ui.label(node_id.fmt_short());
                            match direction {
                                networking::NodeApprovalDirection::Incoming => {
                                    ui.label("Incoming");
                                }
                                networking::NodeApprovalDirection::Outgoing { referrer } => {
                                    ui.label(format!(
                                        "Outgoing (referred to by {})",
                                        referrer.fmt_short()
                                    ));
                                }
                            }

                            let mut retain = true;

                            if ui.button("Allow").clicked() {
                                let _ =
                                    sender.take().unwrap().send(NodeApprovalResponse::Approved(
                                        NodeSharingPolicy::AllExcept(Default::default()),
                                    ));
                                retain = false;
                            }

                            if ui.button("Allow (private)").clicked() {
                                let _ =
                                    sender.take().unwrap().send(NodeApprovalResponse::Approved(
                                        NodeSharingPolicy::NoneExcept(Default::default()),
                                    ));
                                retain = false;
                            }
                            if ui.button("Deny").clicked() {
                                let _ = sender
                                    .take()
                                    .unwrap()
                                    .send(networking::NodeApprovalResponse::Denied);
                                retain = false;
                            }

                            retain
                        })
                        .inner
                    });
                }

                ui.heading("Log");

                log_lines.draw(ui);
            });
        }

        let output = egui.end_frame();
        let meshes = egui.tessellate(output.shapes, output.pixels_per_point);

        // Update usd camera state

        let transform = camera.update(1.0 / 60.0);

        engine.set_camera_state(view_from_camera_transform(transform), proj);

        let usd_state = usd_state.write().await;

        usd_state.xformable_avatar.set_translation(
            transform.position.as_dvec3() - glam::DVec3::new(0.0, 0.2, 0.0),
            Default::default(),
        );

        let usd_state = usd_state.downgrade();

        compare_and_send(&mut state_tx, local_layers.export());

        // Rendering
        unsafe {
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }

        engine.render(&usd_state.pseudo_root, &params);

        drop(usd_state);

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
