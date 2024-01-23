use bbl_usd::{tf, usd, vt};
use dolly::prelude::*;
use glfw::{fail_on_errors, Action, Context, Key};
use glow::HasContext;

fn main() {
    let mut glfw = glfw::init(fail_on_errors!()).unwrap();

    let (mut window, events) = glfw
        .create_window(300, 300, "Hello this is window", glfw::WindowMode::Windowed)
        .expect("Failed to create GLFW window.");

    window.make_current();
    window.set_key_polling(true);

    let gl =
        unsafe { glow::Context::from_loader_function(|s| window.get_proc_address(s) as *const _) };

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

    let mut size = window.get_size();
    engine.set_render_viewport(glam::DVec4::new(0.0, 0.0, size.0 as _, size.1 as _));

    let mut grab_toggled = false;
    let mut prev_cursor_pos = glam::DVec2::from(window.get_cursor_pos());

    while !window.should_close() {
        glfw.poll_events();

        for (_, event) in glfw::flush_messages(&events) {
            match event {
                glfw::WindowEvent::Key(Key::Escape, _, Action::Press, _) => {
                    window.set_should_close(true)
                }
                glfw::WindowEvent::Key(Key::G, _, Action::Press, _) => {
                    grab_toggled = !grab_toggled;
                    if grab_toggled {
                        window.set_cursor_mode(glfw::CursorMode::Disabled);
                    } else {
                        window.set_cursor_mode(glfw::CursorMode::Normal);
                    }
                }
                _ => {}
            }
        }

        let cursor_pos = glam::DVec2::from(window.get_cursor_pos());
        let delta = cursor_pos - prev_cursor_pos;
        prev_cursor_pos = cursor_pos;

        let current_size = window.get_size();
        if current_size != size {
            size = current_size;
            engine.set_render_viewport(glam::DVec4::new(0.0, 0.0, size.0 as _, size.1 as _));
        }

        let mut movement = glam::IVec3::ZERO;

        movement.x += (window.get_key(glfw::Key::D) != glfw::Action::Release) as i32;
        movement.x -= (window.get_key(glfw::Key::A) != glfw::Action::Release) as i32;

        movement.y += (window.get_key(glfw::Key::W) != glfw::Action::Release) as i32;
        movement.y -= (window.get_key(glfw::Key::S) != glfw::Action::Release) as i32;

        movement.z += (window.get_key(glfw::Key::Q) != glfw::Action::Release) as i32;
        movement.z -= (window.get_key(glfw::Key::Z) != glfw::Action::Release) as i32;

        let movement = movement.as_vec3() * 0.1;

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

        let transform = camera.update(1.0 / 60.0);
        let view = glam::DMat4::look_at_rh(
            transform.position.as_dvec3(),
            transform.position.as_dvec3() + transform.forward().as_dvec3(),
            transform.up().as_dvec3(),
        );
        let proj = glam::DMat4::perspective_rh_gl(59.0_f64.to_radians(), 1.0, 0.01, 1000.0);

        engine.set_camera_state(view, proj);

        unsafe {
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }

        engine.render(&prim, &params);

        window.swap_buffers();
    }
}
