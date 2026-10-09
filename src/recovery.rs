use esp_idf_svc::sys;
use node_core::RELAY_GPIO;

// This board is active-HIGH on GPIO25. The ISR must not allocate, log or call flash code.
#[no_mangle]
#[link_section = ".iram1.text"]
pub unsafe extern "C" fn esp_task_wdt_isr_user_handler() {
    std::ptr::write_volatile(sys::GPIO_OUT_W1TC_REG as *mut u32, 1 << RELAY_GPIO);
}

extern "C" fn relay_off_at_shutdown() {
    unsafe { std::ptr::write_volatile(sys::GPIO_OUT_W1TC_REG as *mut u32, 1 << RELAY_GPIO) };
}

pub fn initialize() -> anyhow::Result<()> {
    sys::esp!(unsafe { sys::esp_register_shutdown_handler(Some(relay_off_at_shutdown)) })?;
    log::info!(
        "Boot reset reason: {:?}",
        esp_idf_svc::hal::reset::ResetReason::get()
    );
    Ok(())
}
