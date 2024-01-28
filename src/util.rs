pub fn compare_and_send<T: PartialEq>(sender: &mut tokio::sync::watch::Sender<T>, value: T) {
    sender.send_if_modified(|current| {
        if *current == value {
            return false;
        }
        *current = value;
        true
    });
}

pub fn get_movement(window: &glfw::Window) -> glam::IVec3 {
    let mut movement = glam::IVec3::ZERO;

    movement.x += (window.get_key(glfw::Key::D) != glfw::Action::Release) as i32;
    movement.x -= (window.get_key(glfw::Key::A) != glfw::Action::Release) as i32;

    movement.y += (window.get_key(glfw::Key::W) != glfw::Action::Release) as i32;
    movement.y -= (window.get_key(glfw::Key::S) != glfw::Action::Release) as i32;

    movement.z += (window.get_key(glfw::Key::Q) != glfw::Action::Release) as i32;
    movement.z -= (window.get_key(glfw::Key::Z) != glfw::Action::Release) as i32;

    movement
}

pub fn view_from_camera_transform(
    transform: dolly::transform::Transform<dolly::handedness::RightHanded>,
) -> glam::DMat4 {
    glam::DMat4::look_at_rh(
        transform.position.as_dvec3(),
        transform.position.as_dvec3() + transform.forward().as_dvec3(),
        transform.up().as_dvec3(),
    )
}

pub fn draw_connection(ui: &mut egui::Ui, connection_info: &iroh_net::magicsock::EndpointInfo) {
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

pub fn draw_connection_grid(
    ui: &mut egui::Ui,
    connection_infos: &[iroh_net::magicsock::EndpointInfo],
) {
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
