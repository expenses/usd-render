use ash::vk::{self, Handle as _};
use bbl_usd::{cpp, sdf, tf, usd, vt};
use clap::Parser;
use dolly::prelude::*;
use iroh_net::key::SecretKey;
use std::path::PathBuf;
use std::sync::Arc;
use winit::event::VirtualKeyCode;

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

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Ash - Example")
        .build(&event_loop)
        .unwrap();

    let mut args = Args::parse();

    let hgi_vulkan = usd::HgiVulkan::new();

    let vulkan = unsafe { setup_vulkan(&hgi_vulkan) }?;

    let surface = unsafe {
        ash_window::create_surface(
            &vulkan.entry,
            &vulkan.instance,
            window.raw_display_handle(),
            window.raw_window_handle(),
            None,
        )
    }?;

    let surface_loader = ash::extensions::khr::Surface::new(&vulkan.entry, &vulkan.instance);

    let surface_format = unsafe {
        surface_loader.get_physical_device_surface_formats(vulkan.physical_device, surface)
    }?[0];

    let surface_capabilities = unsafe {
        surface_loader.get_physical_device_surface_capabilities(vulkan.physical_device, surface)
    }?;

    let swapchain_loader = ash::extensions::khr::Swapchain::new(&vulkan.instance, &vulkan.device);

    let initial_size = window.inner_size();

    let mut swapchain_create_info = vk::SwapchainCreateInfoKHR::builder()
        .surface(surface)
        .min_image_count(3)
        .image_color_space(surface_format.color_space)
        .image_format(surface_format.format)
        .image_extent(vk::Extent2D {
            width: initial_size.width as _,
            height: initial_size.height as _,
        })
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(vk::PresentModeKHR::FIFO)
        .clipped(true)
        .image_array_layers(1);

    let mut swapchain = unsafe { Swapchain::new(&swapchain_create_info, &swapchain_loader) }?;

    let mut egui_integration = egui_winit_ash_integration::Integration::new(
        &window,
        initial_size.width as _,
        initial_size.height as _,
        1.0,
        Default::default(),
        Default::default(),
        vulkan.device.clone(),
        Arc::new(std::sync::Mutex::new(
            gpu_allocator::vulkan::Allocator::new(&gpu_allocator::vulkan::AllocatorCreateDesc {
                instance: vulkan.instance.clone(),
                device: vulkan.device.clone(),
                physical_device: vulkan.physical_device.clone(),
                debug_settings: Default::default(),
                buffer_device_address: false,
                allocation_sizes: Default::default(),
            })?,
        )),
        0,
        vulkan.queue.clone(),
        swapchain_loader.clone(),
        swapchain.swapchain.clone(),
        surface_format,
    );

    let engine = usd::GLEngine::new(&hgi_vulkan);
    engine.set_enable_presentation(false);

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

    let params = usd::GLRenderParams::new();
    params.set_cull_style(bbl_usd::ffi::usdImaging_GLCullStyle_usdImaging_GLCullStyle_CULL_STYLE_BACK_UNLESS_DOUBLE_SIDED);
    params.set_enable_lighting(false);

    engine.set_render_viewport(glam::DVec4::new(
        0.0,
        0.0,
        initial_size.width as _,
        initial_size.height as _,
    ));
    engine.set_renderer_aov(&tf::Token::new("color"));

    let mut grab_toggled = false;

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

    let mut keyboard_state = util::KeyboardState::default();

    let _ = event_loop.run(move |event, _target, control_flow| {
        let result = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();

            handle.block_on(async {
                match event {
                    Event::WindowEvent { event, .. } => {
                        let _ = egui_integration.handle_event(&event);

                        match event {
                            WindowEvent::CloseRequested => {
                                *control_flow = ControlFlow::Exit;
                            }
                            WindowEvent::KeyboardInput {
                                input:
                                    winit::event::KeyboardInput {
                                        state,
                                        virtual_keycode: Some(key),
                                        ..
                                    },
                                ..
                            } => {
                                let pressed = state == winit::event::ElementState::Pressed;

                                match key {
                                    VirtualKeyCode::W => keyboard_state.w = pressed,
                                    VirtualKeyCode::A => keyboard_state.a = pressed,
                                    VirtualKeyCode::S => keyboard_state.s = pressed,
                                    VirtualKeyCode::D => keyboard_state.d = pressed,
                                    VirtualKeyCode::Q => keyboard_state.q = pressed,
                                    VirtualKeyCode::Z => keyboard_state.z = pressed,
                                    VirtualKeyCode::G if pressed => {
                                        grab_toggled = !grab_toggled;
                                        if grab_toggled {
                                            if window
                                                .set_cursor_grab(
                                                    winit::window::CursorGrabMode::Locked,
                                                )
                                                .is_err()
                                            {
                                                window.set_cursor_grab(
                                                    winit::window::CursorGrabMode::Confined,
                                                )?;
                                            }
                                            window.set_cursor_visible(false);
                                        } else {
                                            window.set_cursor_grab(
                                                winit::window::CursorGrabMode::None,
                                            )?;
                                            window.set_cursor_visible(true);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            WindowEvent::Resized(new_size) => {
                                swapchain_create_info.image_extent = vk::Extent2D {
                                    width: new_size.width as _,
                                    height: new_size.height as _,
                                };
                                swapchain = unsafe {
                                    Swapchain::new(&swapchain_create_info, &swapchain_loader)
                                }?;
                                engine.set_render_viewport(glam::DVec4::new(
                                    0.0,
                                    0.0,
                                    new_size.width as _,
                                    new_size.height as _,
                                ));
                                egui_integration.update_swapchain(
                                    new_size.width as _,
                                    new_size.height as _,
                                    swapchain.swapchain,
                                    surface_format,
                                );
                            }

                            _ => {}
                        }
                    }
                    Event::DeviceEvent {
                        event: winit::event::DeviceEvent::MouseMotion { delta },
                        ..
                    } => {
                        let delta = glam::DVec2::from(delta);

                        if grab_toggled {
                            let yaw_pitch = -delta * 0.1;
                            camera
                                .driver_mut::<YawPitch>()
                                .rotate_yaw_pitch(yaw_pitch.x as _, yaw_pitch.y as _);
                        }
                    }
                    Event::MainEventsCleared => {
                        window.request_redraw();
                    }
                    Event::RedrawRequested(_) => unsafe {
                        vulkan
                            .device
                            .wait_for_fences(&[vulkan.render_fence], true, u64::MAX)?;

                        vulkan.device.reset_fences(&[vulkan.render_fence])?;

                        vulkan.device.reset_command_pool(
                            vulkan.command_pool,
                            vk::CommandPoolResetFlags::empty(),
                        )?;

                        let swapchain_image_index = match swapchain_loader.acquire_next_image(
                            swapchain.swapchain,
                            u64::MAX,
                            vulkan.present_semaphore,
                            vk::Fence::null(),
                        ) {
                            Ok((swapchain_image_index, _suboptimal)) => swapchain_image_index,
                            Err(error) => {
                                log::warn!("Next frame error: {:?}", error);
                                return Ok(());
                            }
                        };

                        let swapchain_image = swapchain.images[swapchain_image_index as usize];

                        // Egui

                        egui_integration.begin_frame(&window);

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

                            egui::Window::new("Network").show(&egui_integration.context(), |ui| {
                                ui::draw_node_info(ui, &addr, &ticket);

                                ui::draw_connect_to_node(
                                    ui,
                                    &networking_state,
                                    &addr,
                                    &mut ui_state,
                                );

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

                        let output = egui_integration.end_frame(&window);

                        let meshes = egui_integration.context().tessellate(output.shapes);

                        // Update usd camera state

                        let movement = if egui_integration.context().wants_keyboard_input() {
                            glam::IVec3::ZERO
                        } else {
                            util::get_movement(&keyboard_state)
                        }
                        .as_vec3()
                            * 0.1;

                        let movement = movement.x * camera.final_transform.right()
                            + movement.y * camera.final_transform.forward()
                            + movement.z * glam::Vec3::Y;

                        camera.driver_mut::<Position>().translate(movement);

                        let transform = camera.update(1.0 / 60.0);

                        engine.set_camera_state(util::view_from_camera_transform(transform), proj);

                        let usd_state = usd_state.write().await;

                        position_xform_op.set(
                            &vt::Value::from_dvec3(transform.position.as_dvec3()),
                            Default::default(),
                        );
                        rotation_xform_op.set(
                            &vt::Value::from_dquat(
                                transform.rotation.as_f64()
                                    * glam::DQuat::from_rotation_y(180_f64.to_radians()),
                            ),
                            Default::default(),
                        );

                        let usd_state = usd_state.downgrade();

                        {
                            let (index, serialized) = local_layers.export();
                            ipc::compare_and_send_existing_layer(&mut state_tx, serialized, index);
                        }

                        engine.render(&usd_state.pseudo_root, &params);

                        let texture = engine.get_aov_texture(&tf::Token::new("color"));
                        assert_ne!(texture.ptr, std::ptr::null_mut());
                        let vulkan_texture = texture.get_vulkan_texture();
                        let color_image = vk::Image::from_raw(vulkan_texture.get_image());

                        vulkan.device.begin_command_buffer(
                            vulkan.command_buffer,
                            &vk::CommandBufferBeginInfo::builder()
                                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                        )?;

                        blit_image(
                            &vulkan,
                            color_image,
                            swapchain_image,
                            swapchain_create_info.image_extent,
                        );

                        egui_integration.paint(
                            vulkan.command_buffer,
                            swapchain_image_index as _,
                            meshes,
                            output.textures_delta,
                        );

                        vulkan.device.end_command_buffer(vulkan.command_buffer)?;

                        vulkan.device.queue_submit(
                            vulkan.queue,
                            &[*vk::SubmitInfo::builder()
                                .wait_semaphores(&[vulkan.present_semaphore])
                                .wait_dst_stage_mask(&[vk::PipelineStageFlags::TRANSFER])
                                .command_buffers(&[vulkan.command_buffer])
                                .signal_semaphores(&[vulkan.render_semaphore])],
                            vulkan.render_fence,
                        )?;

                        swapchain_loader.queue_present(
                            vulkan.queue,
                            &vk::PresentInfoKHR::builder()
                                .wait_semaphores(&[vulkan.render_semaphore])
                                .swapchains(&[swapchain.swapchain])
                                .image_indices(&[swapchain_image_index]),
                        )?;
                    },
                    _ => {}
                }

                Ok::<_, anyhow::Error>(())
            })
        });

        if let Err(loop_closure) = result {
            log::error!("Error: {}", loop_closure);
        }
    });

    endpoint.close(0_u32.into(), b"user closed").await?;
    Ok(())
}

use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

struct Vulkan {
    device: ash::Device,
    instance: ash::Instance,
    entry: ash::Entry,
    queue: vk::Queue,
    command_buffer: vk::CommandBuffer,
    physical_device: vk::PhysicalDevice,
    render_fence: vk::Fence,
    render_semaphore: vk::Semaphore,
    present_semaphore: vk::Semaphore,
    command_pool: vk::CommandPool,
}

unsafe fn setup_vulkan(hgi: &usd::HgiVulkan) -> anyhow::Result<Vulkan> {
    let entry = ash::Entry::load().unwrap();
    let graphics_queue_family = 0;

    let instance = hgi.get_vulkan_instance();

    let primary_device = hgi.get_primary_device();
    let physical_device =
        ash::vk::PhysicalDevice::from_raw(primary_device.get_vulkan_physical_device());

    let instance = ash::Instance::load(
        &entry.static_fn(),
        ash::vk::Instance::from_raw(instance.get_vulkan_instance()),
    );

    let device = ash::Device::load(
        instance.fp_v1_0(),
        vk::Device::from_raw(primary_device.get_vulkan_device()),
    );

    let present_semaphore =
        unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None) }?;
    let render_semaphore =
        unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None) }?;
    let render_fence = unsafe {
        device.create_fence(
            &vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )?
    };

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

    let command_buffer = command_buffers[0];

    Ok(Vulkan {
        device,
        instance,
        entry,
        queue,
        command_buffer,
        physical_device,
        render_fence,
        render_semaphore,
        present_semaphore,
        command_pool,
    })
}

unsafe fn blit_image(vulkan: &Vulkan, src: vk::Image, dst: vk::Image, extent: vk::Extent2D) {
    vulkan.device.cmd_blit_image(
        vulkan.command_buffer,
        src,
        vk::ImageLayout::GENERAL,
        dst,
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
                vk::Offset3D {
                    x: extent.width as _,
                    y: extent.height as _,
                    z: 1,
                },
            ],
            dst_offsets: [
                Default::default(),
                vk::Offset3D {
                    x: extent.width as _,
                    y: extent.height as _,
                    z: 1,
                },
            ],
        }],
        ash::vk::Filter::NEAREST,
    );
}

struct Swapchain {
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
}

impl Swapchain {
    unsafe fn new(
        create_info: &vk::SwapchainCreateInfoKHR,
        loader: &ash::extensions::khr::Swapchain,
    ) -> anyhow::Result<Self> {
        let swapchain = loader.create_swapchain(create_info, None)?;
        Ok(Self {
            images: loader.get_swapchain_images(swapchain)?,
            swapchain,
        })
    }
}
