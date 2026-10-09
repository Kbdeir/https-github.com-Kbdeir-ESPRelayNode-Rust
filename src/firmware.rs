use anyhow::{anyhow, ensure, Result};
use esp_idf_svc::{
    nvs::{EspDefaultNvs, EspDefaultNvsPartition},
    ota::EspOta,
    sys,
};
use node_core::runtime::Runtime;
use serde_json::json;
use std::{io::Read, sync::atomic::Ordering, thread, time::Duration};

fn filesystem_partition() -> Result<*const sys::esp_partition_t> {
    let partition = unsafe {
        sys::esp_partition_find_first(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY,
            b"littlefs\0".as_ptr().cast(),
        )
    };
    ensure!(!partition.is_null(), "No LittleFS partition");
    Ok(partition)
}
fn inactive_partition() -> Result<*const sys::esp_partition_t> {
    let partition = unsafe { sys::esp_ota_get_next_update_partition(std::ptr::null()) };
    let running = unsafe { sys::esp_ota_get_running_partition() };
    ensure!(
        !partition.is_null() && partition != running,
        "Dual-slot OTA layout required; install it over COM3 first"
    );
    Ok(partition)
}
pub fn status(runtime: &Runtime) -> node_core::web::Reply {
    let slot = inactive_partition().ok();
    node_core::web::Reply::json(
        200,
        json!({"otaSupported":slot.is_some(), "otaSlotBytes":slot.map(|part| unsafe {(*part).size}), "filesystemBytes":filesystem_partition().ok().map(|part|unsafe {(*part).size}), "ready":runtime.ota_confirmed.load(Ordering::Acquire), "busy":runtime.maintenance.load(Ordering::Acquire)}),
    )
}
pub fn confirm_running() -> Result<()> {
    let running = unsafe { sys::esp_ota_get_running_partition() };
    ensure!(!running.is_null(), "No running application partition");
    let mut state = sys::esp_ota_img_states_t_ESP_OTA_IMG_UNDEFINED;
    let result = unsafe { sys::esp_ota_get_state_partition(running, &mut state) };
    if result == 0 && state == sys::esp_ota_img_states_t_ESP_OTA_IMG_PENDING_VERIFY {
        sys::esp!(unsafe { sys::esp_ota_mark_app_valid_cancel_rollback() })?;
        log::info!("OTA image confirmed after 30 seconds of control and management startup");
    }
    Ok(())
}
pub fn download(
    filesystem: bool,
    mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<usize> {
    let partition = if filesystem {
        filesystem_partition()?
    } else {
        unsafe { sys::esp_ota_get_running_partition() }
    };
    ensure!(!partition.is_null(), "No running partition");
    let length = unsafe { (*partition).size } as usize;
    let mut buffer = [0; 768];
    for offset in (0..length).step_by(buffer.len()) {
        let count = (length - offset).min(buffer.len());
        sys::esp!(unsafe {
            sys::esp_partition_read(partition, offset, buffer.as_mut_ptr().cast(), count)
        })?;
        write(&buffer[..count])?;
    }
    Ok(length)
}
pub fn download_length(filesystem: bool) -> Result<usize> {
    let partition = if filesystem {
        filesystem_partition()?
    } else {
        unsafe { sys::esp_ota_get_running_partition() }
    };
    ensure!(!partition.is_null(), "No running partition");
    Ok(unsafe { (*partition).size } as usize)
}
struct Inhibit<'a>(&'a Runtime);
impl Drop for Inhibit<'_> {
    fn drop(&mut self) {
        self.0.maintenance.store(false, Ordering::Release);
    }
}
fn inhibit(runtime: &Runtime) -> Result<Inhibit<'_>> {
    ensure!(
        runtime.restart_at.load(Ordering::Acquire) == 0,
        "Restart already pending"
    );
    ensure!(
        runtime.ota_confirmed.load(Ordering::Acquire),
        "Wait for the 30-second startup health check"
    );
    ensure!(
        !runtime.maintenance.swap(true, Ordering::AcqRel),
        "Maintenance already active"
    );
    let guard = Inhibit(runtime);
    for _ in 0..100 {
        if !runtime.read().relay.on {
            thread::sleep(Duration::from_millis(20));
            return Ok(guard);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(anyhow!("Control did not acknowledge relay OFF"))
}
pub fn upload_firmware(runtime: &Runtime, length: usize, mut reader: impl Read) -> Result<()> {
    let partition = inactive_partition()?;
    ensure!((256..=unsafe {(*partition).size} as usize).contains(&length), "Firmware must fit the inactive OTA slot; upload an application .bin, not a merged flash image");
    let mut header = [0; 24];
    reader.read_exact(&mut header)?;
    ensure!(
        header[0] == 0xe9 && (1..=16).contains(&header[1]) && header[12..14] == [0, 0],
        "Not an ESP32 application image"
    );
    let _guard = inhibit(runtime)?;
    let mut ota = EspOta::new()?;
    let mut update = ota.initiate_update()?;
    update.write(&header)?;
    let mut buffer = [0; 768];
    let mut remaining = length - header.len();
    while remaining != 0 {
        let count = remaining.min(buffer.len());
        reader.read_exact(&mut buffer[..count])?;
        update.write(&buffer[..count])?;
        remaining -= count;
        thread::sleep(Duration::from_millis(1));
    }
    // IDF checks the complete image before activation. Drop aborts an interrupted upload.
    update.finish()?.activate()?;
    runtime
        .restart_at
        .store((runtime.now_ms() / 1000) as u32 + 3, Ordering::Release);
    Ok(())
}
fn crc(partition: *const sys::esp_partition_t, length: usize) -> Result<u32> {
    let mut result = 0u32;
    let mut buffer = [0; 768];
    for offset in (0..length).step_by(buffer.len()) {
        let count = (length - offset).min(buffer.len());
        sys::esp!(unsafe {
            sys::esp_partition_read(partition, offset, buffer.as_mut_ptr().cast(), count)
        })?;
        result = unsafe { sys::esp_rom_crc32_le(result, buffer.as_ptr(), count as u32) };
    }
    Ok(result)
}
struct ReadOnlyMount(*const sys::esp_partition_t);
impl Drop for ReadOnlyMount {
    fn drop(&mut self) {
        unsafe { sys::esp_vfs_littlefs_unregister_partition(self.0) };
    }
}
fn validate_filesystem(partition: *const sys::esp_partition_t, length: usize) -> Result<()> {
    // A descriptor for the image's exact geometry, retained until the temporary mount is dropped.
    let mut view = unsafe { *partition };
    view.size = length as u32;
    let mut config = sys::esp_vfs_littlefs_conf_t::default();
    config.base_path = b"/fs-check\0".as_ptr().cast();
    config.partition = &view;
    config.set_read_only(1);
    config.set_format_if_mount_failed(0);
    sys::esp!(unsafe { sys::esp_vfs_littlefs_register(&config) })?;
    let _mount = ReadOnlyMount(&view);
    for name in [
        "config.html",
        "rust-ui.js",
        "FirmwareMaintenance.html",
        "Backup.html",
        "Files.html",
    ] {
        let mut file = std::fs::File::open(format!("/fs-check/{name}"))?;
        let mut byte = [0];
        ensure!(
            file.read(&mut byte)? == 1,
            "Filesystem is missing a required web asset"
        );
    }
    Ok(())
}
pub fn upload_filesystem(
    runtime: &Runtime,
    nvs: &EspDefaultNvsPartition,
    length: usize,
    mut reader: impl Read,
) -> Result<()> {
    let target = filesystem_partition()?;
    let stage = inactive_partition()?;
    ensure!(
        length == unsafe { (*target).size } as usize && length <= unsafe { (*stage).size } as usize,
        "Filesystem image must exactly match the LittleFS partition size"
    );
    let _guard = inhibit(runtime)?;
    sys::esp!(unsafe { sys::esp_partition_erase_range(stage, 0, length) })?;
    let mut buffer = [0; 768];
    let mut offset = 0;
    while offset < length {
        let count = (length - offset).min(buffer.len());
        reader.read_exact(&mut buffer[..count])?;
        sys::esp!(unsafe {
            sys::esp_partition_write(stage, offset, buffer.as_ptr().cast(), count)
        })?;
        offset += count;
        thread::sleep(Duration::from_millis(1));
    }
    validate_filesystem(stage, length)?;
    let label = unsafe { std::ffi::CStr::from_ptr((*stage).label.as_ptr()) }.to_str()?;
    let bytes =
        serde_json::to_vec(&json!({"slot":label,"length":length,"crc":crc(stage,length)?}))?;
    let mut storage = EspDefaultNvs::new(nvs.clone(), "maintenance", true)?;
    storage.set_blob("fs_stage", &bytes)?;
    runtime
        .restart_at
        .store((runtime.now_ms() / 1000) as u32 + 3, Ordering::Release);
    Ok(())
}
pub fn recover_filesystem(nvs: &EspDefaultNvsPartition) -> Result<()> {
    let mut storage = EspDefaultNvs::new(nvs.clone(), "maintenance", true)?;
    let Some(size) = storage.blob_len("fs_stage")? else {
        return Ok(());
    };
    ensure!(size <= 256, "Invalid filesystem recovery marker");
    let mut bytes = vec![0; size];
    let marker: serde_json::Value = serde_json::from_slice(
        storage
            .get_blob("fs_stage", &mut bytes)?
            .ok_or_else(|| anyhow!("Filesystem marker disappeared"))?,
    )?;
    let label = marker["slot"]
        .as_str()
        .ok_or_else(|| anyhow!("Invalid staging partition label"))?;
    ensure!(matches!(label, "ota_0" | "ota_1"), "Invalid staging slot");
    let label = std::ffi::CString::new(label)?;
    let stage = unsafe {
        sys::esp_partition_find_first(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_APP,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY,
            label.as_ptr(),
        )
    };
    ensure!(
        !stage.is_null() && stage != unsafe { sys::esp_ota_get_running_partition() },
        "Unsafe filesystem staging partition"
    );
    let target = filesystem_partition()?;
    let length = marker["length"]
        .as_u64()
        .ok_or_else(|| anyhow!("Invalid filesystem length"))? as usize;
    ensure!(
        length == unsafe { (*target).size } as usize && length <= unsafe { (*stage).size } as usize,
        "Filesystem recovery geometry mismatch"
    );
    ensure!(
        Some(u64::from(crc(stage, length)?)) == marker["crc"].as_u64(),
        "Filesystem recovery checksum mismatch"
    );
    validate_filesystem(stage, length)?;
    log::info!("Installing validated staged LittleFS image; interrupted installs resume on boot");
    sys::esp!(unsafe { sys::esp_partition_erase_range(target, 0, length) })?;
    let mut buffer = [0; 768];
    for offset in (0..length).step_by(buffer.len()) {
        let count = (length - offset).min(buffer.len());
        sys::esp!(unsafe {
            sys::esp_partition_read(stage, offset, buffer.as_mut_ptr().cast(), count)
        })?;
        sys::esp!(unsafe {
            sys::esp_partition_write(target, offset, buffer.as_ptr().cast(), count)
        })?;
        thread::sleep(Duration::from_millis(1));
    }
    ensure!(
        crc(target, length)? == crc(stage, length)?,
        "Filesystem installation verification failed"
    );
    storage.remove("fs_stage")?;
    Ok(())
}
