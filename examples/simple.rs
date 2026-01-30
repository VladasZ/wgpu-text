#[path = "ctx.rs"]
mod ctx;

use ctx::Ctx;
use glyph_brush::ab_glyph::FontRef;
use glyph_brush::{HorizontalAlign, OwnedSection};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wgpu_text::glyph_brush::{Layout, Section, Text, VerticalAlign};
use wgpu_text::{BrushBuilder, TextBrush};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{self, ActiveEventLoop, ControlFlow};
use winit::window::Window;

struct State<'a> {
    // Use an `Option` to allow the window to not be available until the
    // application is properly running.
    window: Option<Arc<Window>>,
    font: &'a [u8],
    brush: Option<TextBrush<FontRef<'a>>>,
    font_size: f32,
    section_a: Option<OwnedSection>,
    section_b: Option<OwnedSection>,

    target_framerate: Duration,
    delta_time: Instant,
    fps_update_time: Instant,
    fps: i32,

    // wgpu
    ctx: Option<Ctx>,
}

impl ApplicationHandler for State<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("wgpu-text: 'simple' example"),
                )
                .unwrap(),
        );

        self.ctx = Some(Ctx::new(window.clone()));

        let ctx = self.ctx.as_ref().unwrap();
        let device = &ctx.device;
        let config = &ctx.config;

        self.brush = Some(BrushBuilder::using_font_bytes(self.font).unwrap().build(
            device,
            config.width,
            config.height,
            config.format,
        ));

        self.section_a = Some(
            Section::default()
                .add_text(
                    Text::new("------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------")
                        .with_scale(10.0)
                        .with_color([0.0, 0.0, 0.0, 1.0]),
                )
                .with_bounds((config.width as f32, config.height as f32))
                .with_layout(
                    Layout::default()
                        .v_align(VerticalAlign::Center)
                        .h_align(HorizontalAlign::Center),
                )
                .with_screen_position((400.0, 300.0))
                .to_owned(),
        );

        self.section_b = Some(
            Section::default()
                .add_text(
                    Text::new("E█")
                        .with_scale(500.0)
                        .with_color([0.2, 0.5, 0.8, 1.0]),
                )
                .with_bounds((config.width as f32, config.height as f32))
                .with_layout(
                    Layout::default()
                        .v_align(VerticalAlign::Center)
                        .h_align(HorizontalAlign::Center),
                )
                .with_screen_position((400.0, 300.0))
                .to_owned(),
        );

        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        elwt: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(new_size) => {
                let ctx = self.ctx.as_mut().unwrap();
                let queue = &ctx.queue;
                let device = &ctx.device;
                let config = &mut ctx.config;
                let surface = &ctx.surface;
                let brush = self.brush.as_mut().unwrap();

                config.width = new_size.width.max(1);
                config.height = new_size.height.max(1);
                surface.configure(device, config);

                brush.resize_view(config.width as f32, config.height as f32, queue);

                // You can also do this!
                // brush.update_matrix(wgpu_text::ortho(config.width, config.height), &queue);
            }
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::RedrawRequested => {
                let brush = self.brush.as_mut().unwrap();
                let ctx = self.ctx.as_ref().unwrap();
                let queue = &ctx.queue;
                let device = &ctx.device;
                let config = &ctx.config;
                let surface = &ctx.surface;
                let section_a = self.section_a.as_ref().unwrap();
                let section_b = self.section_b.as_ref().unwrap();

                match brush.queue(device, queue, [section_b, section_a]) {
                    Ok(_) => (),
                    Err(err) => {
                        panic!("{err}");
                    }
                };

                let frame = match surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(_) => {
                        surface.configure(device, config);
                        surface
                            .get_current_texture()
                            .expect("Failed to acquire next surface texture!")
                    }
                };
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Command Encoder"),
                    });

                {
                    let mut rpass =
                        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Render Pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.2,
                                        g: 0.2,
                                        b: 0.3,
                                        a: 1.,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });

                    brush.draw(&mut rpass);
                }

                queue.submit([encoder.finish()]);
                frame.present();
            }
            _ => (),
        }
    }

    fn new_events(&mut self, _elwt: &ActiveEventLoop, _cause: winit::event::StartCause) {
        if self.target_framerate <= self.delta_time.elapsed()
            && let Some(window) = self.window.clone().as_mut()
        {
            window.request_redraw();
            self.delta_time = Instant::now();
            self.fps += 1;
            if self.fps_update_time.elapsed().as_millis() > 1000 {
                window.set_title(&format!(
                    "wgpu-text: 'simple' example, FPS: {}",
                    self.fps
                ));
                self.fps = 0;
                self.fps_update_time = Instant::now();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        println!("Exiting!");
    }
}

// TODO text layout of characters like 'š, ć, ž, đ' doesn't work correctly.
fn main() {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
    }
    env_logger::init();

    let event_loop = event_loop::EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut state = State {
        window: None,
        font: include_bytes!("fonts/DejaVuSans.ttf"),
        brush: None,
        font_size: 25.,
        section_a: None,
        section_b: None,

        // FPS and window updating:
        // change '60.0' if you want different FPS cap
        target_framerate: Duration::from_secs_f64(1.0 / 60.0),
        delta_time: Instant::now(),
        fps_update_time: Instant::now(),
        fps: 0,

        ctx: None,
    };

    let _ = event_loop.run_app(&mut state);
}
