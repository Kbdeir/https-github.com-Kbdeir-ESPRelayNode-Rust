//! Phase 0: ESP-IDF bring-up on the original ESP32.
//! Network services and relay outputs are deliberately not enabled yet.
use anyhow::Result;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("ESPRelayNode-Rust: Phase 0 boot OK");

    loop {
        log::info!("ESPRelayNode-Rust heartbeat");
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}
