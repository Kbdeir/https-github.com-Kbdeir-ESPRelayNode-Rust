mod app;
mod filesystem;
mod firmware;
mod http;
mod modbus;
mod mqtt;
mod recovery;
mod services;
mod storage;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    if let Err(error) = app::run() {
        log::error!("Control stopped: {error}; relay OFF, restarting after 60 seconds");
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(60_000);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }
    Ok(())
}
