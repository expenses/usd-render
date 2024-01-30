use bbl_usd::{cpp, sdf, usd, vt};
use clap::Parser;
use dolly::prelude::*;
use glfw::{Action, Context, Key};
use glow::HasContext;
use iroh_net::{key::SecretKey, ticket::NodeTicket};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

mod layers;
mod logging;
mod networking;
mod util;

use layers::LocalLayers;
use networking::{NodeApprovalResponse, NodeSharingPolicy};
use util::*;

const ALPN: &[u8] = b"myalpn";

struct UsdState {
    stage: usd::StageRefPtr,
    root_layer: sdf::LayerHandle,
    pseudo_root: usd::Prim,
}

#[derive(Parser, Debug)]
struct Args {
    base: String,
    avatar: String,
    #[arg(long)]
    keyfile: Option<PathBuf>,
    #[arg(long)]
    peers_data: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();

    logging::setup()?;

    let approved_nodes = networking::ApprovedNodes::default();
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::channel(10);
    let connected_nodes = networking::ConnectedNodes::default();

    let secret_key = match args.keyfile {
        Some(keyfile) => SecretKey::try_from_openssh(std::fs::read(keyfile)?)?,
        None => SecretKey::generate(),
    };

    let mut endpoint_builder = iroh_net::MagicEndpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_owned()]);

    if let Some(peers_data_path) = args.peers_data.take() {
        endpoint_builder = endpoint_builder.peers_data_path(peers_data_path);
    }

    let endpoint = endpoint_builder.bind(0).await?;

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

    let base_layer = sdf::Layer::find_or_open(&args.base);

    root_layer.insert_sub_layer_path(base_layer.get_identifier(), 0);

    let mut local_layers = LocalLayers::new(&root_layer);

    let prim = stage.pseudo_root();

    local_layers.set_public_edit_target(&stage);

    // note: prefix with _avatar as names can't start with numbers.
    let avatar = stage
        .define_prim(
            &format!("/avatars/avatar_{}", endpoint.node_id().fmt_short())[..],
            "Xform",
        )
        .map_err(|err| anyhow::anyhow!("{:?}", err))?;

    let xformable = usd::Xformable::new(&avatar);

    let position_xform_op =
        xformable.add_xform_op(bbl_usd::ffi::usdGeom_XformOpType_usdGeom_XformOpType_TypeTranslate);
    let rotation_xform_op =
        xformable.add_xform_op(bbl_usd::ffi::usdGeom_XformOpType_usdGeom_XformOpType_TypeOrient);

    let mut references = avatar.get_references();
    references.add_reference(&cpp::String::new(&args.avatar));

    local_layers.set_private_edit_target(&stage);

    {
        xformable
            .add_xform_op(bbl_usd::ffi::usdGeom_XformOpType_usdGeom_XformOpType_TypeScale)
            .set(
                &vt::Value::from_dvec3(glam::DVec3::ZERO),
                Default::default(),
            );
    }

    let (mut state_tx, state_rx) = tokio::sync::watch::channel((0_u8, cpp::String::default()));

    compare_and_send(&mut state_tx, local_layers.export());

    local_layers.add_new_sublayer();

    local_layers.set_public_edit_target(&stage);

    let usd_state = Arc::new(tokio::sync::RwLock::new(UsdState {
        root_layer,
        pseudo_root: prim,
        stage,
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
    params.set_cull_style(bbl_usd::ffi::usdImaging_GLCullStyle_usdImaging_GLCullStyle_CULL_STYLE_BACK_UNLESS_DOUBLE_SIDED);

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

                ui.collapsing("Connections", |ui| {
                    draw_connection_grid(ui, &connection_infos);
                });

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

                ui.collapsing("Log", |ui| {
                    log_lines.draw(ui);
                });

                if ui.button("export").clicked() {
                    tokio::spawn({
                        let networking_state = networking_state.clone();
                        async move {
                            let state = networking_state.usd.read().await;
                            state.stage.export(&cpp::String::new("export.usdc"));
                        }
                    });
                }
                if ui.button("save keyfile").clicked() {
                    tokio::spawn({
                        let endpoint = networking_state.endpoint.clone();
                        async move {
                            let function = || async move {
                                let key = endpoint.secret_key();
                                let serialized = key.to_openssh()?;
                                // None if cancelled.
                                if let Some(filehandle) =
                                    rfd::AsyncFileDialog::new().save_file().await
                                {
                                    filehandle.write(serialized.as_bytes()).await?;
                                }
                                Ok::<_, anyhow::Error>(())
                            };

                            if let Err(error) = function().await {
                                log::error!("{}", error);
                            }
                        }
                    });
                }
            });
        }

        let output = egui.end_frame();
        let meshes = egui.tessellate(output.shapes, output.pixels_per_point);

        // Update usd camera state

        let transform = camera.update(1.0 / 60.0);

        engine.set_camera_state(view_from_camera_transform(transform), proj);

        let usd_state = usd_state.write().await;

        position_xform_op.set(
            &vt::Value::from_dvec3(transform.position.as_dvec3()),
            Default::default(),
        );
        rotation_xform_op.set(
            &vt::Value::from_dquat(
                transform.rotation.as_f64() * glam::DQuat::from_rotation_y(180_f64.to_radians()),
            ),
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

    endpoint.close(0_u32.into(), b"user closed").await?;

    Ok(())
}
