use glow::HasContext;

fn main() {
    let (gl, window, event_loop) = unsafe {
        let event_loop = glutin::event_loop::EventLoop::new();
        let window_builder = glutin::window::WindowBuilder::new()
            .with_title("Hello triangle!")
            .with_inner_size(glutin::dpi::LogicalSize::new(1024.0, 768.0));
        let window = glutin::ContextBuilder::new()
            .with_vsync(true)
            .build_windowed(window_builder, &event_loop)
            .unwrap()
            .make_current()
            .unwrap();
        let gl = glow::Context::from_loader_function(|s| window.get_proc_address(s) as *const _);
        (gl, window, event_loop)
    };

    let mut imaging = std::ptr::null_mut();

    dbg!(unsafe { bbl_usd::ffi::usdImaging_GLEngine_new(&mut imaging) });

    let stage = bbl_usd::usd::Stage::open(std::env::args().nth(1).unwrap()).unwrap();

    let prim = stage.pseudo_root();
    let camera_prim = stage.prim_at_path("/camera1").unwrap();

    dbg!(imaging);

    unsafe {
        use glutin::event::{Event, WindowEvent};
        use glutin::event_loop::ControlFlow;

        gl.clear_color(0.1, 0.2, 0.3, 1.0);

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::LoopDestroyed => {
                    return;
                }
                Event::MainEventsCleared => {
                    window.window().request_redraw();
                }
                Event::RedrawRequested(_) => {
                    gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

                    bbl_usd::ffi::usdImaging_set_camera(imaging, camera_prim.ptr());

                    let viewport = glam::DVec4::new(
                        0.0,
                        0.0,
                        window.window().inner_size().width as _,
                        window.window().inner_size().height as _,
                    );

                    bbl_usd::ffi::usdImaging_GLEngine_SetRenderViewport(
                        imaging,
                        &viewport as *const glam::DVec4 as *const _,
                    );

                    bbl_usd::ffi::usdImaging_render(imaging, prim.ptr());

                    dbg!(window.window().inner_size());
                    window.swap_buffers().unwrap();
                }
                Event::WindowEvent { ref event, .. } => match event {
                    WindowEvent::Resized(physical_size) => {
                        window.resize(*physical_size);
                    }
                    WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                    _ => (),
                },
                _ => (),
            }
        });
    }
}
