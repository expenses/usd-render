use bbl_usd::usd;
use dolly::prelude::*;
use glfw::{Action, Context, Key};
use glow::HasContext;
use std::sync::Arc;

use egui_window_glfw_passthrough::{GlfwBackend, GlfwConfig};

fn main() {
    let mut glfw_backend = GlfwBackend::new(GlfwConfig {
        ..Default::default()
    });

    glfw_backend.window.make_current();
    glfw_backend.window.set_key_polling(true);

    let gl = unsafe {
        glow::Context::from_loader_function(|s| glfw_backend.window.get_proc_address(s) as *const _)
    };

    let gl = Arc::new(gl);

    let egui = egui::Context::default();

    let mut painter = egui_glow::painter::Painter::new(gl.clone(), "", None).unwrap();

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

    while !glfw_backend.window.should_close() {
        glfw_backend.glfw.poll_events();
        glfw_backend.tick();

        // Handle key presses
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

        let movement = get_movement(&glfw_backend.window).as_vec3() * 0.1;

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

        egui.begin_frame(glfw_backend.take_raw_input());

        {
            egui::Window::new("My Window").show(&egui, |ui| {
                ui.label(&format!("{:?}", camera.final_transform.position));
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
