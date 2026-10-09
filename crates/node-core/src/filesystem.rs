use serde::Serialize;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const CAPACITY: u64 = 0xc0000;
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_FILES: usize = 128;
type Usage = fn() -> io::Result<(u64, u64)>;

pub struct FileStore {
    root: PathBuf,
    usage: Option<Usage>,
    mutation: Mutex<()>,
}
#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub size: u64,
}

pub fn valid_path(path: &str) -> bool {
    let Some(name) = path.strip_prefix('/') else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
pub fn asset_path(path: &str) -> &str {
    match path {
        "/" => "/config.html",
        "/Timer1" => "/Timer1.html",
        "/Automation" => "/Automation.html",
        "/Files" => "/Files.html",
        "/Input_Relays_Map" => "/Input_Relays_Map.html",
        _ => path,
    }
}
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
impl FileStore {
    pub fn new(root: impl Into<PathBuf>, usage: Option<Usage>) -> Self {
        Self {
            root: root.into(),
            usage,
            mutation: Mutex::new(()),
        }
    }
    pub(crate) fn path(&self, name: &str) -> io::Result<PathBuf> {
        if !valid_path(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid file path",
            ));
        }
        let path = self.root.join(&name[1..]);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Not a regular file",
                ));
            }
        }
        Ok(path)
    }
    pub fn open(&self, name: &str) -> io::Result<File> {
        File::open(self.path(name)?)
    }
    pub fn list(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = format!("/{}", entry.file_name().to_string_lossy());
            if !valid_path(&name) || !entry.file_type()?.is_file() {
                continue;
            }
            if entries.len() == MAX_FILES {
                return Err(io::Error::other("File count limit exceeded"));
            }
            entries.push(Entry {
                name,
                size: entry.metadata()?.len(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    pub fn info(&self) -> io::Result<(u64, u64)> {
        if let Some(usage) = self.usage {
            return usage();
        }
        Ok((
            CAPACITY,
            self.list()?
                .iter()
                .map(|e| e.size.div_ceil(4096) * 4096)
                .sum(),
        ))
    }
    pub fn upload(&self, name: &str, length: usize, mut reader: impl Read) -> io::Result<()> {
        let _guard = self.mutation.lock().unwrap();
        let destination = self.path(name)?;
        if length > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File exceeds 256 KiB limit",
            ));
        }
        if !destination.exists() && self.list()?.len() >= MAX_FILES {
            return Err(io::Error::other("File count limit exceeded"));
        }
        let (total, used) = self.info()?;
        if total.saturating_sub(used) < length as u64 + 8192 {
            return Err(io::Error::other(
                "Insufficient free storage for atomic upload",
            ));
        }
        let temporary = self.root.join(".upload.tmp");
        let result = (|| {
            let mut file = File::create(&temporary)?;
            let mut buffer = [0; 768];
            let mut remaining = length;
            while remaining > 0 {
                let count = remaining.min(buffer.len());
                reader.read_exact(&mut buffer[..count])?;
                file.write_all(&buffer[..count])?;
                remaining -= count;
            }
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn delete(&self, name: &str) -> io::Result<()> {
        let _guard = self.mutation.lock().unwrap();
        fs::remove_file(self.path(name)?)
    }
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let _guard = self.mutation.lock().unwrap();
        let source = self.path(from)?;
        let destination = self.path(to)?;
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination already exists",
            ));
        }
        fs::rename(source, destination)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn exclusive(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutation.lock().unwrap()
    }
}
