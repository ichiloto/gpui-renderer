mod app;
mod assets;
mod canvas;
mod canvas_protocol;
#[cfg(test)]
mod canvas_tests;
mod color;
mod diagnostics;
mod input;
mod protocol;
mod render_trace;
mod renderer;
mod state;
mod tile_regions;
mod tile_sampling;
mod tiles;
mod transport;
mod viewport;
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
