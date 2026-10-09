use anyhow::Result;
use esp_idf_svc::sys;
use node_core::{filesystem::FileStore, runtime::Runtime};
use std::{io, sync::Arc};

fn usage() -> io::Result<(u64, u64)> {
    let mut total = 0;
    let mut used = 0;
    let result =
        unsafe { sys::esp_littlefs_info(b"littlefs\0".as_ptr().cast(), &mut total, &mut used) };
    if result != 0 {
        return Err(io::Error::other(format!("LittleFS info error {result}")));
    }
    Ok((total as u64, used as u64))
}
pub struct Mounted;
impl Drop for Mounted {
    fn drop(&mut self) {
        let result = unsafe { sys::esp_vfs_littlefs_unregister(b"littlefs\0".as_ptr().cast()) };
        if result != 0 {
            log::warn!("LittleFS unmount error {result}");
        }
    }
}
pub fn mount(runtime: &Runtime) -> Result<Mounted> {
    let mut config = sys::esp_vfs_littlefs_conf_t::default();
    config.base_path = b"/littlefs\0".as_ptr().cast();
    config.partition_label = b"littlefs\0".as_ptr().cast();
    // Never auto-format an unreadable filesystem: preserve uploaded assets for recovery.
    config.set_format_if_mount_failed(0);
    sys::esp!(unsafe { sys::esp_vfs_littlefs_register(&config) })?;
    let mounted = Mounted;
    let (total, used) = usage()?;
    let store = Arc::new(FileStore::new("/littlefs", Some(usage)));
    node_core::backup::recover(&store)?;
    *runtime.files.lock().unwrap() = Some(store);
    log::info!("LittleFS mounted: {used}/{total} bytes; web assets served from partition");
    Ok(mounted)
}
