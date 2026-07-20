#[path = "../wifi_backend/mod.rs"]
mod wifi_backend;

fn main() {
    if let Err(error) = wifi_backend::run() {
        eprintln!("a26-wifi-service: {error}");
        std::process::exit(1);
    }
}
