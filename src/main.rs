use bbl_usd::{cpp, sdf, tf, usd, vt};
use clap::Parser;
use dolly::prelude::*;
use glfw::{Action, Context, Key};
use glow::HasContext;
use iroh_net::key::SecretKey;
use std::path::PathBuf;
use std::sync::Arc;

mod ipc;
mod layers;
mod logging;
mod networking;
mod ui;
mod util;

use layers::LocalLayers;

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
/*
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
    engine.set_renderer_aov(&tf::Token::new("color"));

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

    let (mut state_tx, state_rx) = tokio::sync::watch::channel(ipc::PublicLayerState {
        layers: vec![local_layers.export().1],
        updated_layer: 0,
        update_index: 0,
    });

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
    params.set_color_correction_mode(&tf::Token::new("sRGB"));
    params.set_enable_lighting(false);

    let mut size = glfw_backend.window.get_size();
    engine.set_render_viewport(glam::DVec4::new(0.0, 0.0, size.0 as _, size.1 as _));

    let mut grab_toggled = false;
    let mut prev_cursor_pos = glam::DVec2::from(glfw_backend.window.get_cursor_pos());

    let proj = glam::DMat4::perspective_rh_gl(59.0_f64.to_radians(), 1.0, 0.01, 1000.0);

    let addr = endpoint.my_addr().await?;

    let networking_state = networking::State {
        endpoint: endpoint.clone(),
        approved_nodes: approved_nodes.clone(),
        approval_queue: approval_tx.clone(),
        connected_nodes: connected_nodes.clone(),
        state: state_rx.clone(),
        usd: usd_state.clone(),
    };

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

    let mut ui_state = ui::State::default();

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
        let current_size = glfw_backend.window.get_framebuffer_size();
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
            util::get_movement(&glfw_backend.window)
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
            while !ui_state.approval_queue.is_full() {
                if let Ok(request) = approval_rx.try_recv() {
                    ui_state.approval_queue.push((
                        request.node_id,
                        request.direction,
                        Some(request.response_sender),
                    ));
                } else {
                    break;
                }
            }

            let connection_infos = endpoint.connection_infos().await?;
            let log_lines = logging::get_lines().await;

            egui::Window::new("Network").show(&egui, |ui| {
                ui::draw_node_info(ui, &addr, &ticket, &mut glfw_backend.window);

                ui::draw_connect_to_node(ui, &networking_state, &addr, &mut ui_state);

                ui.collapsing("Connections", |ui| {
                    ui::draw_connection_grid(ui, &connection_infos);
                });

                if !ui_state.approval_queue.is_empty() {
                    ui::draw_approval_queue(ui, &mut ui_state, &approved_nodes);
                }

                ui.collapsing("Log", |ui| {
                    log_lines.draw(ui);
                });

                ui::draw_buttons(ui, &networking_state);
            });
        }

        let output = egui.end_frame();
        let meshes = egui.tessellate(output.shapes, output.pixels_per_point);

        // Update usd camera state

        let transform = camera.update(1.0 / 60.0);

        engine.set_camera_state(util::view_from_camera_transform(transform), proj);

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

        {
            let (index, serialized) = local_layers.export();
            ipc::compare_and_send_existing_layer(&mut state_tx, serialized, index);
        }

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

    painter.destroy();

    endpoint.close(0_u32.into(), b"user closed").await?;

    Ok(())
}
*/

use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};
use winit::{
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use ash::vk;

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().unwrap();
    let window = WindowBuilder::new()
        .with_title("Ash - Example")
        .build(&event_loop)
        .unwrap();

    use ash::vk::Handle;

    unsafe {
        let mut args = Args::parse();

        let hgi_vulkan = usd::HgiVulkan::new();

        let entry = ash::Entry::load().unwrap();

        let instance = hgi_vulkan.get_vulkan_instance();

        let primary_device = hgi_vulkan.get_primary_device();
        let pdevice =
            ash::vk::PhysicalDevice::from_raw(primary_device.get_vulkan_physical_device());

        let raw_instance = instance.get_vulkan_instance();
        let instance = ash::Instance::load(
            &entry.static_fn(),
            ash::vk::Instance::from_raw(raw_instance),
        );

        let device = ash::Device::load(
            instance.fp_v1_0(),
            vk::Device::from_raw(primary_device.get_vulkan_device()),
        );

        let surface = ash_window::create_surface(
            &entry,
            &instance,
            window.raw_display_handle(),
            window.raw_window_handle(),
            None,
        )
        .unwrap();

        let surface_loader = ash::extensions::khr::Surface::new(&entry, &instance);

        let surface_format = surface_loader
            .get_physical_device_surface_formats(pdevice, surface)
            .unwrap()[0];

        let surface_capabilities = surface_loader
            .get_physical_device_surface_capabilities(pdevice, surface)
            .unwrap();

        let swapchain_loader = ash::extensions::khr::Swapchain::new(&instance, &device);

        let mut swapchain_create_info = vk::SwapchainCreateInfoKHR::builder()
            .surface(surface)
            .min_image_count(3)
            .image_color_space(surface_format.color_space)
            .image_format(surface_format.format)
            .image_extent(vk::Extent2D {
                width: 512,
                height: 512,
            })
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .image_array_layers(1);

        let mut swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None)?;

        let engine = usd::GLEngine::new(&hgi_vulkan);
        engine.set_enable_presentation(false);
        //engine.set_renderer_aov(&tf::Token::new("color"));
        
        let mut camera: CameraRig = CameraRig::builder()
            .with(Position::new(glam::Vec3::splat(2.0)))
            .with(YawPitch {
                yaw_degrees: 45.0,
                pitch_degrees: -45.0,
            })
            .with(Smooth::new_position_rotation(1.0, 0.1))
            .build();

        let stage = usd::Stage::create_in_memory();
        // Root layer that holds all other layers.
        let root_layer = stage.get_root_layer();

        let base_layer = sdf::Layer::find_or_open(&args.base);

        root_layer.insert_sub_layer_path(base_layer.get_identifier(), 0);

        let prim = stage.pseudo_root();
        let params = usd::GLRenderParams::new();
        params.set_cull_style(bbl_usd::ffi::usdImaging_GLCullStyle_usdImaging_GLCullStyle_CULL_STYLE_BACK_UNLESS_DOUBLE_SIDED);
        //params.set_color_correction_mode(&tf::Token::new("sRGB"));
        //params.set_enable_lighting(false);

        let proj = glam::DMat4::perspective_rh_gl(59.0_f64.to_radians(), 1.0, 0.01, 1000.0);
        engine.set_renderer_aov(&tf::Token::new("color"));

        let transform = camera.update(1.0 / 60.0);

        let graphics_queue_family = 0;

        let present_semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None) }?;
        let render_semaphore = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None) }?;
        let render_fence = unsafe { device.create_fence(&vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED), None)? };

        let queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };

        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder().queue_family_index(graphics_queue_family),
                None,
            )
        }?;

        let command_buffers = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }?;

        let mut swapchain_images = swapchain_loader.get_swapchain_images(swapchain)?;

        let command_buffer = command_buffers[0];
        let _ = event_loop.run(move |event, elwt| {
            let loop_closure = || -> anyhow::Result<()> {
                match event {
                    Event::WindowEvent { event, .. } => match event {
                        WindowEvent::CloseRequested => {
                            elwt.exit();
                        }

                        WindowEvent::Resized(new_size) => {
                            swapchain_create_info.image_extent = vk::Extent2D {
                                width: new_size.width as _,
                                height: new_size.height as _,
                            };
                            swapchain =
                                swapchain_loader.create_swapchain(&swapchain_create_info, None)?;
                            swapchain_images = swapchain_loader.get_swapchain_images(swapchain)?;
                        }
                        WindowEvent::RedrawRequested => unsafe {
                            device.wait_for_fences(&[render_fence], true, u64::MAX)?;

                            device.reset_fences(&[render_fence])?;

                            device.reset_command_pool(
                                command_pool,
                                vk::CommandPoolResetFlags::empty(),
                            )?;

                            let swapchain_image_index = match swapchain_loader.acquire_next_image(
                                swapchain,
                                u64::MAX,
                                present_semaphore,
                                vk::Fence::null(),
                            ) {
                                Ok((swapchain_image_index, _suboptimal)) => swapchain_image_index,
                                Err(error) => {
                                    log::warn!("Next frame error: {:?}", error);
                                    return Ok(());
                                }
                            };

                            let swapchain_image = swapchain_images[swapchain_image_index as usize];

                            let size = window.inner_size();
                            engine.set_render_viewport(glam::DVec4::new(
                                0.0,
                                0.0,
                                size.width as _,
                                size.height as _,
                            ));

                            engine.set_camera_state(
                                util::view_from_camera_transform(transform),
                                proj,
                            );
                            engine.render(&prim, &params);

                            let texture = engine.get_aov_texture(&tf::Token::new("color"));
                            assert_ne!(texture.ptr, std::ptr::null_mut());

                            let mut vulkan_texture = std::ptr::null_mut();
                            bbl_usd::ffi::usdImaging_get_vulkan_texture(
                                texture.ptr,
                                &mut vulkan_texture,
                            );
                            assert_ne!(vulkan_texture, std::ptr::null_mut());

                            let mut vulkan_texture_raw = 0;
                            bbl_usd::ffi::usdImaging_HgiVulkanTexture_GetImage(
                                vulkan_texture,
                                &mut vulkan_texture_raw,
                            );
                            let vulkan_texture = vk::Image::from_raw(vulkan_texture_raw);

                            device.begin_command_buffer(
                                command_buffer,
                                &vk::CommandBufferBeginInfo::builder()
                                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                            )?;

                            let dim = vk::Offset3D {
                                x: swapchain_create_info.image_extent.width as _,
                                y: swapchain_create_info.image_extent.height as _,
                                z: 1
                            };

                            device.cmd_blit_image(
                                command_buffer,
                                vulkan_texture,
                                vk::ImageLayout::GENERAL,
                                swapchain_image,
                                vk::ImageLayout::GENERAL,
                                &[ash::vk::ImageBlit {
                                    src_subresource: ash::vk::ImageSubresourceLayers {
                                        mip_level: 0,
                                        base_array_layer: 0,
                                        layer_count: 1,
                                        aspect_mask: ash::vk::ImageAspectFlags::COLOR,
                                    },
                                    dst_subresource: ash::vk::ImageSubresourceLayers {
                                        mip_level: 0,
                                        base_array_layer: 0,
                                        layer_count: 1,
                                        aspect_mask: ash::vk::ImageAspectFlags::COLOR,
                                    },
                                    src_offsets: [
                                        Default::default(),
                                        dim,
                                    ],
                                    dst_offsets: [
                                        Default::default(),
                                        dim,
                                    ],
                                }],
                                ash::vk::Filter::NEAREST,
                            );

                            device.end_command_buffer(command_buffer)?;

                            device.queue_submit(
                                queue,
                                &[*vk::SubmitInfo::builder()
                                    .wait_semaphores(&[present_semaphore])
                                    .wait_dst_stage_mask(&[vk::PipelineStageFlags::TRANSFER])
                                    .command_buffers(&[command_buffer])
                                    .signal_semaphores(&[render_semaphore])],
                                render_fence,
                            )?;

                            swapchain_loader.queue_present(
                                queue,
                                &vk::PresentInfoKHR::builder()
                                    .wait_semaphores(&[render_semaphore])
                                    .swapchains(&[swapchain])
                                    .image_indices(&[swapchain_image_index]),
                            )?;
                        },
                        _ => {}
                    },
                    _ => {}
                }

                Ok(())
            };

            if let Err(loop_closure) = loop_closure() {
                log::error!("Error: {}", loop_closure);
            }

            window.request_redraw();
        });
    }

    Ok(())
}
