mod bindings;

use bindings::*;

static mut IMAGING: *mut bbl_usd::ffi::usdImaging_GLEngine_t = std::ptr::null_mut();
static mut PRIM: *const bbl_usd::ffi::usd_Prim_t = std::ptr::null();

fn main() {
    let window = unsafe {
        glutInit(&mut 0, std::ptr::null_mut());
        glutInitDisplayMode(GLUT_RGBA);
        glutInitWindowSize(500, 500);
        glutInitWindowPosition(0, 0);
        glutCreateWindow(std::ffi::CStr::from_bytes_with_nul(b"GLLLL\0").unwrap().as_ptr())
    };

    let mut imaging = std::ptr::null_mut();

    dbg!(unsafe { bbl_usd::ffi::usdImaging_GLEngine_new(&mut imaging) });

    let stage = bbl_usd::usd::Stage::open(std::env::args().nth(1).unwrap()).unwrap();

    let prim = stage.pseudo_root();
    let camera_prim = stage.prim_at_path("/camera1").unwrap();

    dbg!(imaging);

    unsafe {
        IMAGING = imaging;
        PRIM = prim.ptr();
    }

    let mut camera = std::ptr::null_mut();
    let mut gf_camera = std::ptr::null_mut();
    let mut frustum = std::ptr::null_mut();
    let mut proj = bbl_usd::ffi::gf_Matrix4d_t {
        m: Default::default(),
    };

    let mut view = bbl_usd::ffi::gf_Matrix4d_t {
        m: Default::default(),
    };

    unsafe {
        bbl_usd::ffi::usdGeom_Camera_new(camera_prim.ptr(), &mut camera);
        bbl_usd::ffi::usdGeom_Camera_GetCamera(
            camera,
            &bbl_usd::ffi::usd_TimeCode_t {
                time: Default::default()
            },
            &mut gf_camera,
        );
        bbl_usd::ffi::gf_Camera_GetFrustum(gf_camera, &mut frustum);
        bbl_usd::ffi::gf_Frustum_ComputeProjectionMatrix(frustum, &mut proj);
        bbl_usd::ffi::gf_Frustum_ComputeViewMatrix(frustum, &mut view);
    }

    dbg!(camera, gf_camera);



    unsafe {
        glClearColor(0.1, 0.2, 0.3, 1.0);

        bbl_usd::ffi::usdImaging_GLEngine_SetCameraState(imaging, &view, &proj);

        unsafe extern "C" fn showScreen() {
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

            let viewport = glam::DVec4::new(
                0.0,
                0.0,
                glutGet(GLUT_WINDOW_WIDTH) as f64,
                glutGet(GLUT_WINDOW_HEIGHT) as f64,
            );
            bbl_usd::ffi::usdImaging_GLEngine_SetRenderViewport(
                IMAGING,
                &viewport as *const glam::DVec4 as *const _,
            );
            bbl_usd::ffi::usdImaging_render(IMAGING, PRIM);


            glutSwapBuffers();
        }

        glutDisplayFunc(Some(showScreen));
        glutIdleFunc(Some(showScreen));
        glutMainLoop();
    }


    /*unsafe {
        event_loop.run(move |event, target| {
            target.set_control_flow(ControlFlow::Wait);
            match event {
                Event::WindowEvent { ref event, .. } => match event {
                    WindowEvent::Resized(physical_size) => {
                        display.resize((physical_size.width, physical_size.height));
                    }
                    WindowEvent::CloseRequested => {
                        target.set_control_flow(ControlFlow::Wait)
                    },
                    WindowEvent::Destroyed => {
                        return;
                    },
                    WindowEvent::RedrawRequested => {
                        let mut frame = display.draw();
                        frame.clear(None, Some((0.1, 0.2, 0.3, 1.0)), false, None, None);


                        let viewport = glam::DVec4::new(
                            0.0,
                            0.0,
                            window.inner_size().width as _,
                            window.inner_size().height as _,
                        );

                        bbl_usd::ffi::usdImaging_GLEngine_SetRenderViewport(
                            imaging,
                            &viewport as *const glam::DVec4 as *const _,
                        );

                        bbl_usd::ffi::usdImaging_render(imaging, prim.ptr());

                        // /dbg!(window.window().inner_size());
                        //window.swap_buffers().unwrap();
                        frame.finish();
                        //panic!();
                    }
                    _ => (),
                },
                Event::AboutToWait => {
                    window.request_redraw();
                },
                _ => (),
            }
        });
    }*/
}
