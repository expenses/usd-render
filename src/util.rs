#[derive(Default)]
pub struct KeyboardState {
    pub w: bool,
    pub a: bool,
    pub s: bool,
    pub d: bool,
    pub q: bool,
    pub z: bool,
}

pub fn get_movement(state: &KeyboardState) -> glam::IVec3 {
    let mut movement = glam::IVec3::ZERO;

    movement.x += state.d as i32;
    movement.x -= state.a as i32;

    movement.y += state.w as i32;
    movement.y -= state.s as i32;

    movement.z += state.q as i32;
    movement.z -= state.z as i32;

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

pub fn spawn_fallible<
    F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    H: FnOnce(anyhow::Error) -> HF + Send + 'static,
    HF: std::future::Future<Output = ()> + Send,
>(
    future: F,
    handler: H,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(err) = future.await {
            handler(err).await;
        }
    })
}
