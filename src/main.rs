use std::error::Error;
use std::ffi::{CStr, CString};
use std::num::NonZeroU32;
use std::ops::Deref;

use raw_window_handle::HasRawWindowHandle;
use winit::event::{Event, KeyEvent, WindowEvent};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::SwapInterval;

use glutin_winit::{self, DisplayBuilder, GlWindow};

pub mod gl {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));

    pub use Gles2 as Gl;
}

use winit::event_loop::EventLoopBuilder;

pub fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoopBuilder::new().build().unwrap();

    // Only Windows requires the window to be present before creating the display.
    // Other platforms don't really need one.
    //
    // XXX if you don't care about running on Android or so you can safely remove
    // this condition and always pass the window builder.
    let window_builder = cfg!(wgl_backend).then(|| {
        WindowBuilder::new()
            .with_title("Glutin triangle gradient example (press Escape to exit)")
    });

    // The template will match only the configurations supporting rendering
    // to windows.3
    //
    // XXX We force transparency only on macOS, given that EGL on X11 doesn't
    // have it, but we still want to show window. The macOS situation is like
    // that, because we can query only one config at a time on it, but all
    // normal platforms will return multiple configs, so we can find the config
    // with transparency ourselves inside the `reduce`.
    let template = ConfigTemplateBuilder::new()
        .with_alpha_size(8)
        .with_transparency(cfg!(cgl_backend));

    let display_builder = DisplayBuilder::new().with_window_builder(window_builder);

    let (mut window, gl_config) = display_builder.build(&event_loop, template, |configs| {
        // Find the config with the maximum number of samples, so our triangle will
        // be smooth.
        configs
            .reduce(|accum, config| {
                let transparency_check = config.supports_transparency().unwrap_or(false)
                    & !accum.supports_transparency().unwrap_or(false);

                if transparency_check || config.num_samples() > accum.num_samples() {
                    config
                } else {
                    accum
                }
            })
            .unwrap()
    })?;

    println!("Picked a config with {} samples", gl_config.num_samples());

    let raw_window_handle = window.as_ref().map(|window| window.raw_window_handle());

    // XXX The display could be obtained from any object created by it, so we can
    // query it from the config.
    let gl_display = gl_config.display();

    // The context creation part. It can be created before surface and that's how
    // it's expected in multithreaded + multiwindow operation mode, since you
    // can send NotCurrentContext, but not Surface.
    let context_attributes = ContextAttributesBuilder::new().build(raw_window_handle);
    let mut not_current_gl_context = Some(unsafe {
        gl_display
            .create_context(&gl_config, &context_attributes)
            .unwrap()
    });

    let stage = bbl_usd::usd::Stage::open(std::env::args().nth(1).unwrap()).unwrap();

    let prim = stage.pseudo_root();
    let camera_prim = stage.prim_at_path("/camera1").unwrap();

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
                time: Default::default(),
            },
            &mut gf_camera,
        );
        bbl_usd::ffi::gf_Camera_GetFrustum(gf_camera, &mut frustum);
        bbl_usd::ffi::gf_Frustum_ComputeProjectionMatrix(frustum, &mut proj);
        bbl_usd::ffi::gf_Frustum_ComputeViewMatrix(frustum, &mut view);
    }

    let mut state = None;
    let mut renderer = None;
    event_loop.run(move |event, window_target| {
        match event {
            Event::Resumed => {
                #[cfg(android_platform)]
                println!("Android window available");

                let window = window.take().unwrap_or_else(|| {
                    let window_builder = WindowBuilder::new()
                        .with_transparent(true)
                        .with_title("Glutin triangle gradient example (press Escape to exit)");
                    glutin_winit::finalize_window(window_target, window_builder, &gl_config)
                        .unwrap()
                });

                let attrs = window.build_surface_attributes(Default::default());
                let gl_surface = unsafe {
                    gl_config
                        .display()
                        .create_window_surface(&gl_config, &attrs)
                        .unwrap()
                };

                // Make it current.
                let gl_context = not_current_gl_context
                    .take()
                    .unwrap()
                    .make_current(&gl_surface)
                    .unwrap();

                // The context needs to be current for the Renderer to set up shaders and
                // buffers. It also performs function loading, which needs a current context on
                // WGL.
                renderer.get_or_insert_with(|| Renderer::new(&gl_display, view, proj));

                // Try setting vsync.
                if let Err(res) = gl_surface
                    .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()))
                {
                    eprintln!("Error setting vsync: {res:?}");
                }

                assert!(state.replace((gl_context, gl_surface, window)).is_none());
            }
            Event::Suspended => {
                // This event is only raised on Android, where the backing NativeWindow for a GL
                // Surface can appear and disappear at any moment.
                println!("Android window removed");

                // Destroy the GL Surface and un-current the GL Context before ndk-glue releases
                // the window back to the system.
                let (gl_context, ..) = state.take().unwrap();
                assert!(not_current_gl_context
                    .replace(gl_context.make_not_current().unwrap())
                    .is_none());
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(size) => {
                    if size.width != 0 && size.height != 0 {
                        // Some platforms like EGL require resizing GL surface to update the size
                        // Notable platforms here are Wayland and macOS, other don't require it
                        // and the function is no-op, but it's wise to resize it for portability
                        // reasons.
                        if let Some((gl_context, gl_surface, _)) = &state {
                            gl_surface.resize(
                                gl_context,
                                NonZeroU32::new(size.width).unwrap(),
                                NonZeroU32::new(size.height).unwrap(),
                            );
                            let renderer = renderer.as_ref().unwrap();
                            renderer.resize(size.width as i32, size.height as i32);
                        }
                    }
                }
                WindowEvent::CloseRequested
                | WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            logical_key: Key::Named(NamedKey::Escape),
                            ..
                        },
                    ..
                } => window_target.exit(),
                _ => (),
            },
            Event::AboutToWait => {
                if let Some((gl_context, gl_surface, window)) = &state {
                    let renderer = renderer.as_ref().unwrap();
                    renderer.draw(&prim);
                    window.request_redraw();

                    gl_surface.swap_buffers(gl_context).unwrap();
                }
            }
            _ => (),
        }
    })?;

    Ok(())
}

pub struct Renderer {
    gl: gl::Gl,
    engine: *mut bbl_usd::ffi::usdImaging_GLEngine_t,
}

impl Renderer {
    pub fn new<D: GlDisplay>(
        gl_display: &D,
        view: bbl_usd::ffi::gf_Matrix4d_t,
        proj: bbl_usd::ffi::gf_Matrix4d_t,
    ) -> Self {
        unsafe {
            let gl = gl::Gl::load_with(|symbol| {
                let symbol = CString::new(symbol).unwrap();
                gl_display.get_proc_address(symbol.as_c_str()).cast()
            });

            let mut engine = std::ptr::null_mut();

            bbl_usd::ffi::usdImaging_GLEngine_new(&mut engine);
            bbl_usd::ffi::usdImaging_GLEngine_SetCameraState(engine, &view, &proj);

            gl.ClearColor(0.1, 0.2, 0.3, 1.0);

            Self { gl, engine }
        }
    }

    pub fn draw(&self, prim: &bbl_usd::usd::Prim) {
        unsafe {
            self.gl.Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
            bbl_usd::ffi::usdImaging_render(self.engine, prim.ptr());
        }
    }

    pub fn resize(&self, width: i32, height: i32) {
        unsafe {
            self.gl.Viewport(0, 0, width, height);

            let viewport = glam::DVec4::new(0.0, 0.0, width as _, height as _);

            bbl_usd::ffi::usdImaging_GLEngine_SetRenderViewport(
                self.engine,
                &viewport as *const glam::DVec4 as *const _,
            );
        }
    }
}
