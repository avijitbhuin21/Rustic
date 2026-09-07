#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rustic_agent::tools::pdf_worker::run_worker_if_requested();
    rustic_lib::run();
}
