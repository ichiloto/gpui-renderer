use crate::protocol::{Event, ProtocolWriter, Version, diagnostic};
use crate::renderer::Renderer;
use crate::state::RendererState;
use crate::transport::{self, Update};
use gpui::{App, AppContext, Application, TitlebarOptions, WindowHandle, WindowOptions};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Output {
    pub diagnostics: crate::diagnostics::Diagnostics,
    pub version: Version,
    pub writer: ProtocolWriter,
    pub failed: Arc<AtomicBool>,
    pub closing: Arc<AtomicBool>,
    pub done: async_channel::Receiver<Result<(), String>>,
}

impl Output {
    pub fn emit_key(
        &self,
        event: Event,
        trace: Option<crate::diagnostics::KeyTrace>,
        cx: &mut App,
    ) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        if let Err(error) = self.writer.send_traced(self.version, event, trace) {
            self.failed.store(true, Ordering::SeqCst);
            diagnostic(error);
            self.stop(cx);
        }
    }

    pub fn emit(&self, event: Event, cx: &mut App) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        if let Err(error) = self.writer.send(self.version, event) {
            self.failed.store(true, Ordering::SeqCst);
            diagnostic(error);
            self.stop(cx);
        }
    }

    fn fatal(&self, message: String, cx: &mut App) {
        self.failed.store(true, Ordering::SeqCst);
        self.emit(Event::Error { message }, cx);
        self.stop(cx);
    }

    fn stop(&self, cx: &mut App) {
        if self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        self.writer.close();
        let output = self.clone();
        // macOS App::quit terminates the process instead of returning to main.
        // Finish IPC and choose the exit status BEFORE handing shutdown to GPUI.
        cx.spawn(async move |cx| {
            let result = futures_lite::future::race(
                async {
                    output
                        .done
                        .recv()
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                },
                async {
                    gpui::Timer::after(std::time::Duration::from_secs(2)).await;
                    Err("stdout did not drain within 2 seconds".into())
                },
            )
            .await;
            if let Err(error) = result {
                diagnostic(format!("protocol output shutdown failed: {error}"));
                output.failed.store(true, Ordering::SeqCst);
            }
            output.diagnostics.clock_anchor("shutdown");
            let trace_result = futures_lite::future::race(output.diagnostics.finish(), async {
                gpui::Timer::after(std::time::Duration::from_secs(2)).await;
                Err("stderr diagnostics did not drain within 2 seconds".into())
            })
            .await;
            if trace_result.is_err() {
                output.failed.store(true, Ordering::SeqCst);
            }
            if output.failed.load(Ordering::SeqCst) {
                std::process::exit(1);
            }
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

pub fn run(output: Output, writer_failure: async_channel::Receiver<String>) {
    let updates = transport::start_reader(output.diagnostics.clone());
    Application::new().run(move |cx: &mut App| {
        let closed_output = output.clone();
        cx.on_window_closed(move |cx| {
            if cx.windows().is_empty() {
                closed_output.stop(cx);
            }
        })
        .detach();
        let failure_output = output.clone();
        cx.spawn(async move |cx| {
            if let Ok(error) = writer_failure.recv().await {
                let _ = cx.update(|cx| {
                    failure_output.failed.store(true, Ordering::SeqCst);
                    diagnostic(format!("stdout write failed: {error}"));
                    failure_output.stop(cx);
                });
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            let mut window: Option<WindowHandle<Renderer>> = None;
            let mut output = output;
            while let Ok(update) = updates.recv().await {
                output
                    .diagnostics
                    .frame_queue_stage(update.observation(), "dequeued", || {
                        (updates.len(), updates.capacity())
                    });
                let stop = matches!(update, Update::Shutdown | Update::Fatal(_) | Update::Eof);
                let result = cx.update(|cx| match update {
                    Update::Hello(version, hello) => {
                        let capabilities = hello.get_enabled_capabilities(version);
                        output.version = version;
                        if let Some(icon) = &hello.icon
                            && let Err(error) = crate::app_icon::apply(&hello.asset_root, icon)
                        {
                            crate::protocol::diagnostic(format!(
                                "keeping the renderer icon: {error}"
                            ));
                        }
                        let grid = hello.grid;
                        let initial = match crate::window_layout::initial_window(
                            grid,
                            cx,
                            &output.diagnostics,
                        ) {
                            Ok(initial) => initial,
                            Err(error) => {
                                output.fatal(error, cx);
                                return;
                            }
                        };
                        let close_output = output.clone();
                        let view_output = output.clone();
                        match cx.open_window(
                            WindowOptions {
                                window_bounds: Some(initial.bounds),
                                display_id: Some(initial.display_id),
                                titlebar: Some(TitlebarOptions {
                                    title: Some(hello.title.clone().into()),
                                    ..Default::default()
                                }),
                                is_resizable: true,
                                ..Default::default()
                            },
                            move |window, cx| {
                                window.on_window_should_close(cx, move |_, cx| {
                                    if close_output.closing.load(Ordering::SeqCst) {
                                        return true;
                                    }
                                    close_output.emit(Event::CloseRequested, cx);
                                    close_output.stop(cx);
                                    false
                                });
                                cx.new(|cx| {
                                    let focus = cx.focus_handle();
                                    window.focus(&focus);
                                    Renderer {
                                        state: RendererState { hello, frame: None },
                                        focus,
                                        output: view_output,
                                        last_viewport: None,
                                        last_logical_size: None,
                                        last_device_scale: None,
                                        cached_images: vec![],
                                        logical_font_size: None,
                                        font_metrics: None,
                                        canvas_fonts: Default::default(),
                                        tile_samples: Default::default(),
                                        glyph_fonts: Default::default(),
                                        glyph_frame: Default::default(),
                                        glyph_density: None,
                                        retained_scene: None,
                                        retained_viewport: None,
                                        retained_observation: None,
                                        retained_needs_reset: false,
                                        activation: Default::default(),
                                        activation_subscription: None,
                                        held: Default::default(),
                                    }
                                })
                            },
                        ) {
                            Ok(handle) => {
                                window = Some(handle);
                                cx.activate(true);
                                output.emit(Event::Ready { capabilities }, cx);
                                if let Err(error) = handle.update(cx, |view, window, cx| {
                                    view.observe_window_activation(window, cx)
                                }) {
                                    output.fatal(
                                        format!("cannot observe window activation: {error}"),
                                        cx,
                                    );
                                }
                            }
                            Err(error) => {
                                output.fatal(format!("cannot open native window: {error}"), cx)
                            }
                        }
                    }
                    #[cfg(test)]
                    Update::Frame(frame) => {
                        if let Some(window) = window {
                            let number = frame.number;
                            if let Err(error) = window.update(cx, |view, window, cx| {
                                let observation = frame.observation;
                                let (width, height) = frame.canvas.as_ref().map_or(
                                    (
                                        (view.state.hello.grid.columns
                                            * view.state.hello.grid.cell_width)
                                            as f32,
                                        (view.state.hello.grid.rows
                                            * view.state.hello.grid.cell_height)
                                            as f32,
                                    ),
                                    |canvas| {
                                        (canvas.source.width as f32, canvas.source.height as f32)
                                    },
                                );
                                let viewport = window.viewport_size();
                                let fit = crate::viewport::ViewportTransform::fit(
                                    width,
                                    height,
                                    viewport.width.into(),
                                    viewport.height.into(),
                                );
                                let density = fit.scale.max(1.0 / 256.0) * window.scale_factor();
                                let prepared = crate::glyph_cache::prepare(
                                    &frame,
                                    density,
                                    &mut view.glyph_fonts,
                                    &mut view.tile_samples.borrow_mut(),
                                );
                                let glyphs = match prepared {
                                    Ok(glyphs) => glyphs,
                                    Err(message) => {
                                        view.output.emit(Event::Error { message }, cx);
                                        return;
                                    }
                                };
                                view.output.diagnostics.geometry("glyph_rasters", || {
                                    vec![
                                        ("frame", number as f64),
                                        ("density", density as f64),
                                        ("builds", glyphs.builds as f64),
                                        ("reserved_bytes", glyphs.reserved_bytes as f64),
                                    ]
                                });
                                view.glyph_frame = glyphs;
                                view.glyph_density = Some(density);
                                view.output
                                    .diagnostics
                                    .frame_stage(observation, "replace_begin");
                                view.state.replace(*frame);
                                view.output.diagnostics.frame_stage(observation, "replaced");
                                cx.notify();
                            }) {
                                output.fatal(format!("cannot present frame {number}: {error}"), cx);
                            }
                        }
                    }
                    Update::RetainedFrame(frame) => {
                        if let Some(window) = window {
                            let generation = frame.generation;
                            let number = frame.frame;
                            if let Err(error) = window.update(cx, |view, window, cx| {
                                if view.retained_needs_reset && !frame.reset {
                                    view.output.emit(
                                        Event::FrameRejected {
                                            generation,
                                            expected_generation: generation,
                                            message:
                                                "native presentation requires a retained reset"
                                                    .into(),
                                            resync_required: true,
                                        },
                                        cx,
                                    );
                                    return;
                                }
                                let screen = &frame.scene.screen;
                                let grid = view.state.hello.grid;
                                let (width, height) = screen.canvas.as_ref().map_or(
                                    (
                                        (grid.columns * grid.cell_width) as f32,
                                        (grid.rows * grid.cell_height) as f32,
                                    ),
                                    |canvas| {
                                        (canvas.source.width as f32, canvas.source.height as f32)
                                    },
                                );
                                let viewport = window.viewport_size();
                                let fit = crate::viewport::ViewportTransform::fit(
                                    width,
                                    height,
                                    viewport.width.into(),
                                    viewport.height.into(),
                                );
                                let density = fit.scale.max(1.0 / 256.0) * window.scale_factor();
                                let glyphs = match crate::glyph_cache::prepare(
                                    screen,
                                    density,
                                    &mut view.glyph_fonts,
                                    &mut view.tile_samples.borrow_mut(),
                                ) {
                                    Ok(glyphs) => glyphs,
                                    Err(message) => {
                                        view.retained_needs_reset = true;
                                        view.output.emit(
                                            Event::FrameRejected {
                                                generation,
                                                expected_generation: generation,
                                                message,
                                                resync_required: true,
                                            },
                                            cx,
                                        );
                                        return;
                                    }
                                };
                                view.glyph_frame = glyphs;
                                view.glyph_density = Some(density);
                                view.output
                                    .diagnostics
                                    .frame_stage(frame.observation, "replace_begin");
                                view.state.replace_shared(screen.clone());
                                view.retained_scene = Some(frame.scene.clone());
                                view.retained_viewport = frame.viewport.clone();
                                view.retained_observation = frame.observation;
                                view.retained_needs_reset = false;
                                view.output
                                    .diagnostics
                                    .frame_stage(frame.observation, "replaced");
                                cx.notify();
                                view.output.emit(
                                    Event::FrameAck {
                                        generation,
                                        frame: number,
                                        presented: true,
                                    },
                                    cx,
                                );
                            }) {
                                output.fatal(
                                    format!("cannot present retained frame {number}: {error}"),
                                    cx,
                                );
                            }
                        }
                    }
                    Update::RetainedAck { generation, frame } => {
                        output.emit(
                            Event::FrameAck {
                                generation,
                                frame,
                                presented: false,
                            },
                            cx,
                        );
                    }
                    Update::RetainedRejected {
                        generation,
                        expected_generation,
                        message,
                    } => {
                        output.emit(
                            Event::FrameRejected {
                                generation,
                                expected_generation,
                                message,
                                resync_required: true,
                            },
                            cx,
                        );
                    }
                    Update::Error(message) => {
                        output.emit(Event::Error { message }, cx);
                    }
                    Update::Fatal(message) => output.fatal(message, cx),
                    Update::Shutdown => output.stop(cx),
                    Update::Eof => {
                        if window.is_none() {
                            output.fatal("stdin closed before a successful hello".into(), cx);
                        }
                        // Once initialized, keep the native window and keyboard usable at EOF.
                    }
                });
                if result.is_err() || stop || output.closing.load(Ordering::SeqCst) {
                    break;
                }
            }
        })
        .detach();
    });
}
