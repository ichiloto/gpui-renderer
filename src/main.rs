mod app;
mod assets;
mod input;
mod protocol;
mod renderer;
mod state;
mod transport;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn main() -> std::process::ExitCode {
    let (writer, completion) = protocol::ProtocolWriter::start();
    let failed = Arc::new(AtomicBool::new(false));
    app::run(
        app::Output {
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
