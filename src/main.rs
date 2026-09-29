mod app;
mod app_icon;
mod assets;
mod canvas;
#[cfg(test)]
mod canvas_clip_tests;
mod canvas_image;
#[cfg(test)]
mod canvas_image_tests;
mod canvas_protocol;
#[cfg(test)]
mod canvas_tests;
mod color;
mod composite_cache;
mod composite_pixels;
mod composite_protocol;
#[cfg(test)]
mod composite_tests;
mod diagnostics;
mod display_cache;
mod field_motion;
mod glyph_cache;
mod glyph_effects;
mod glyph_pixels;
mod glyph_raster;
#[cfg(test)]
mod glyph_tests;
mod input;
mod protocol;
mod render_trace;
mod renderer;
mod retained_paint;
mod retained_prepared;
mod retained_protocol;
mod retained_state;
mod retained_tileset;
mod retained_world;
mod state;
mod tile_regions;
mod tile_sampling;
mod tiles;
mod transport;
mod viewport;
mod window_activation;
mod window_layout;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn main() -> std::process::ExitCode {
    let (writer, completion) = protocol::ProtocolWriter::start();
    let failed = Arc::new(AtomicBool::new(false));
    app::run(
        app::Output {
            diagnostics: diagnostics::Diagnostics::from_environment(),
            version: protocol::Version::V1,
            writer,
            failed: failed.clone(),
            closing: Arc::new(AtomicBool::new(false)),
            done: completion.done,
        },
        completion.failure,
    );
    if failed.load(Ordering::SeqCst) {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
