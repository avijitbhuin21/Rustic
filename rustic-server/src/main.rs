//! `rustic-server` binary entry point — boots the headless web transport.
//! All logic lives in the library (`rustic_server::run`) so it is testable.

#[tokio::main]
async fn main() {
    rustic_agent::tools::pdf_worker::run_worker_if_requested();
    if let Err(e) = rustic_server::run().await {
        eprintln!("[rustic-server] fatal: {e}");
        std::process::exit(1);
    }
}
