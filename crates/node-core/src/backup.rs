use crate::{
    config::{Config, MAX_CONFIG_BYTES},
    filesystem::{FileStore, MAX_FILES, MAX_FILE_BYTES},
    runtime::Runtime,
    web::{Body, Reply},
};
use base64::{engine::general_purpose::STANDARD, read::DecoderReader, write::EncoderWriter};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Write},
    sync::{atomic::Ordering, Arc},
};
use struson::reader::{JsonReader, JsonStreamReader, ReaderSettings};

pub const MAX_BACKUP_BYTES: usize = 2 * 1024 * 1024;
pub const CONFIG_FILE: &str = "/rust-config.json";

pub struct Backup {
    pub config: Box<Config>,
    pub store: Option<Arc<FileStore>>,
    pub chipid: String,
    pub include_web: bool,
}
struct Output<F>(F);
impl<F: FnMut(&[u8]) -> io::Result<()>> Write for Output<F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for chunk in bytes.chunks(768) {
            (self.0)(chunk)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Backup {
    pub fn write(&self, write: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        let mut output = io::BufWriter::with_capacity(768, Output(write));
        let _guard = self.store.as_ref().map(|store| store.exclusive());
        let files = match &self.store {
            Some(store) => store
                .list()?
                .into_iter()
                .filter(|entry| {
                    entry.name != CONFIG_FILE && (self.include_web || entry.name.ends_with(".json"))
                })
                .collect::<Vec<_>>(),
            None if self.include_web => return Err(io::Error::other("LittleFS unavailable")),
            None => Vec::new(),
        };
        write!(
            output,
            "{{\"_backup_version\":1,\"_chipid\":{},\"_includesWeb\":{},\"_files\":{},\"{}\":\"",
            serde_json::to_string(&self.chipid)?,
            self.include_web,
            files.len() + 1,
            CONFIG_FILE
        )?;
        let mut encoder = EncoderWriter::new(&mut output, &STANDARD);
        serde_json::to_writer(&mut encoder, &self.config)?;
        encoder.finish()?;
        drop(encoder);
        output.write_all(b"\"")?;
        for entry in files {
            write!(output, ",{}:\"", serde_json::to_string(&entry.name)?)?;
            let mut encoder = EncoderWriter::new(&mut output, &STANDARD);
            copy_bounded(
                self.store.as_ref().unwrap().open(&entry.name)?,
                &mut encoder,
                MAX_FILE_BYTES,
            )?;
            encoder.finish()?;
            drop(encoder);
            output.write_all(b"\"")?;
        }
        output.write_all(b"}")?;
        output.flush()
    }
}
pub fn download(runtime: &Arc<Runtime>, include_web: bool) -> Reply {
    Reply {
        status: 200,
        mime: "application/json",
        headers: vec![(
            "Content-Disposition",
            "attachment; filename=\"smartconfig-backup.json\"".into(),
        )],
        body: Body::Backup(Box::new(Backup {
            config: Box::new(runtime.config.lock().unwrap().clone()),
            store: runtime.files.lock().unwrap().clone(),
            chipid: runtime.node_id.clone(),
            include_web,
        })),
    }
}

// This is only a resource-limit filter for names. Struson validates all JSON syntax.
struct NameLimited<'a, R> {
    reader: R,
    quoted: bool,
    escaped: bool,
    name: bool,
    count: usize,
    received: &'a mut usize,
}
impl<R: Read> Read for NameLimited<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(buffer)?;
        *self.received += count;
        for &byte in &buffer[..count] {
            if self.quoted {
                self.count += 1;
                if self.name && self.count > 256 {
                    return Err(io::Error::other("Backup member name too long"));
                }
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.quoted = false;
                }
            } else {
                match byte {
                    b'"' => {
                        self.quoted = true;
                        self.count = 0;
                    }
                    b'{' | b',' => self.name = true,
                    b':' => self.name = false,
                    _ => {}
                }
            }
        }
        Ok(count)
    }
}
fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
fn copy_bounded(mut reader: impl Read, writer: &mut impl Write, limit: usize) -> io::Result<usize> {
    let mut buffer = [0; 768];
    let mut total = 0;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        total += count;
        if total > limit {
            return Err(invalid("Backup entry exceeds size limit"));
        }
        writer.write_all(&buffer[..count])?;
    }
}
fn limited_text(reader: impl Read, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    copy_bounded(reader, &mut bytes, limit)?;
    String::from_utf8(bytes).map_err(invalid)
}

#[derive(Serialize, Deserialize)]
struct JournalEntry {
    name: String,
    had_old: bool,
    replace: bool,
}

fn clean_stage(store: &FileStore) -> io::Result<()> {
    let stage = store.root().join(".restore");
    if !stage.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&stage)? {
        fs::remove_file(entry?.path())?;
    }
    fs::remove_dir(stage)
}
pub fn recover(store: &FileStore) -> io::Result<()> {
    let journal = store.root().join(".restore-journal.json");
    if journal.exists() {
        let mut bytes = Vec::new();
        copy_bounded(File::open(&journal)?, &mut bytes, 32768)?;
        let entries: Vec<JournalEntry> = serde_json::from_slice(&bytes).map_err(invalid)?;
        if entries.len() > MAX_FILES * 2 {
            return Err(invalid("Restore journal count exceeds limit"));
        }
        for (index, entry) in entries.iter().enumerate() {
            let destination = store.path(&entry.name)?;
            let old = store.root().join(format!(".restore/{index}.old"));
            if old.exists() {
                if destination.exists() {
                    fs::remove_file(&destination)?;
                }
                fs::rename(old, destination)?;
            } else if !entry.had_old && destination.exists() {
                fs::remove_file(destination)?;
            }
        }
        fs::remove_file(journal)?;
    }
    let _ = fs::remove_file(store.root().join(".restore-journal.tmp"));
    clean_stage(store)
}
fn commit_web(store: &FileStore, names: &[String]) -> io::Result<()> {
    let mut entries: Vec<_> = names
        .iter()
        .map(|name| {
            Ok(JournalEntry {
                name: name.clone(),
                had_old: store.path(name)?.exists(),
                replace: true,
            })
        })
        .collect::<io::Result<_>>()?;
    for entry in store.list()? {
        if !entry.name.ends_with(".json") && !names.contains(&entry.name) {
            entries.push(JournalEntry {
                name: entry.name,
                had_old: true,
                replace: false,
            });
        }
    }
    let journal = store.root().join(".restore-journal.json");
    let temporary = store.root().join(".restore-journal.tmp");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer(&mut file, &entries)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, &journal)?;
    let result = (|| {
        for (index, entry) in entries.iter().enumerate() {
            let destination = store.path(&entry.name)?;
            if entry.had_old {
                fs::rename(
                    &destination,
                    store.root().join(format!(".restore/{index}.old")),
                )?;
            }
            if entry.replace {
                fs::rename(
                    store.root().join(format!(".restore/{index}.new")),
                    destination,
                )?;
            }
        }
        fs::remove_file(&journal)?;
        clean_stage(store)
    })();
    if result.is_err() && journal.exists() {
        recover(store)?;
    }
    result
}
pub fn rewrite_controller_ids(text: &str, target: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while let Some(index) = remaining.find("Controller") {
        let start = index + "Controller".len();
        result.push_str(&remaining[..start]);
        let id_len = remaining[start..]
            .bytes()
            .take_while(u8::is_ascii_hexdigit)
            .count();
        if matches!(id_len, 6 | 12) {
            result.push_str(target);
            remaining = &remaining[start + id_len..];
        } else {
            remaining = &remaining[start..];
        }
    }
    result.push_str(remaining);
    result
}
fn rewrite_config(config: &mut Config, target: &str) {
    for topic in [
        &mut config.relay.command_topic,
        &mut config.relay.state_topic,
        &mut config.relay.ttl_topic,
        &mut config.relay.ttl_command_topic,
        &mut config.relay.elapsed_topic,
    ] {
        *topic = rewrite_controller_ids(topic, target);
    }
    for input in &mut config.inputs {
        input.topic = rewrite_controller_ids(&input.topic, target);
    }
    for sensor in &mut config.remote_sensors {
        sensor.topic = rewrite_controller_ids(&sensor.topic, target);
    }
}

pub fn restore<F>(
    runtime: &Arc<Runtime>,
    uri: &str,
    length: usize,
    reader: impl Read,
    mut persist: F,
) -> Reply
where
    F: FnMut(&Config, &Config) -> Result<(), String>,
{
    let result = restore_inner(runtime, uri, length, reader, &mut persist);
    match result {
        Ok(count) => Reply::json(
            200,
            serde_json::json!({"ok":true,"message":format!("{count} item(s) restored. Restarting with relay OFF."),"restart":true}),
        ),
        Err(error) => Reply::error(
            if error.kind() == io::ErrorKind::InvalidInput {
                400
            } else {
                500
            },
            "Restore rejected: ".to_owned() + &error.to_string(),
        ),
    }
}
fn restore_inner<F>(
    runtime: &Arc<Runtime>,
    uri: &str,
    length: usize,
    reader: impl Read,
    persist: &mut F,
) -> io::Result<usize>
where
    F: FnMut(&Config, &Config) -> Result<(), String>,
{
    if length == 0 || length > MAX_BACKUP_BYTES {
        return Err(invalid("Backup must be between 1 byte and 2 MiB"));
    }
    if runtime.restart_at.load(Ordering::Acquire) != 0 {
        return Err(invalid("Restart already pending"));
    }
    let fields = crate::management::form(
        uri.split_once('?')
            .map_or(b"".as_slice(), |(_, query)| query.as_bytes()),
    )
    .map_err(invalid)?;
    let mode = fields.get("chipIdMode").map_or("keep", String::as_str);
    if !matches!(mode, "keep" | "replace" | "original") {
        return Err(invalid("Invalid controller ID mode"));
    }
    let web = match fields.get("restoreWeb").map_or("0", String::as_str) {
        "0" => false,
        "1" => true,
        _ => return Err(invalid("Invalid restoreWeb flag")),
    };
    let store = runtime
        .files
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| io::Error::other("LittleFS unavailable"))?;
    let _guard = store.exclusive();
    recover(&store)?;
    fs::create_dir(store.root().join(".restore"))?;
    let result = (|| {
        let mut received = 0;
        let input = NameLimited {
            reader: reader.take(length as u64),
            quoted: false,
            escaped: false,
            name: true,
            count: 0,
            received: &mut received,
        };
        let mut json = JsonStreamReader::new_custom(
            input,
            ReaderSettings {
                max_nesting_depth: Some(2),
                track_path: false,
                ..Default::default()
            },
        );
        json.begin_object().map_err(invalid)?;
        let mut seen = BTreeSet::new();
        let mut version = None;
        let mut chipid = String::new();
        let mut includes_web = false;
        let mut declared_count = None;
        let mut names = Vec::new();
        let mut config_bytes = None;
        let mut file_count = 0;
        let mut staged_bytes = 0;
        let (total, used) = store.info()?;
        while json.has_next().map_err(invalid)? {
            let key = json.next_name_owned().map_err(invalid)?;
            if seen.len() >= MAX_FILES + 5 || !seen.insert(key.clone()) {
                return Err(invalid("Duplicate key or too many backup entries"));
            }
            match key.as_str() {
                "_backup_version" => {
                    version = Some(
                        json.next_number::<u32>()
                            .map_err(invalid)?
                            .map_err(invalid)?,
                    )
                }
                "_chipid" => {
                    chipid = limited_text(json.next_string_reader().map_err(invalid)?, 64)?
                }
                "_includesWeb" => includes_web = json.next_bool().map_err(invalid)?,
                "_files" => {
                    declared_count = Some(
                        json.next_number::<usize>()
                            .map_err(invalid)?
                            .map_err(invalid)?,
                    )
                }
                _ => {
                    if !crate::filesystem::valid_path(&key) {
                        return Err(invalid("Invalid backup file path"));
                    }
                    file_count += 1;
                    let mut decoder =
                        DecoderReader::new(json.next_string_reader().map_err(invalid)?, &STANDARD);
                    if key == CONFIG_FILE {
                        let mut bytes = Vec::new();
                        copy_bounded(&mut decoder, &mut bytes, MAX_CONFIG_BYTES)?;
                        config_bytes = Some(bytes);
                    } else if web && !key.ends_with(".json") {
                        let mut file = File::create(
                            store.root().join(format!(".restore/{}.new", names.len())),
                        )?;
                        let available = total.saturating_sub(used + staged_bytes + 16384) as usize;
                        let count =
                            copy_bounded(&mut decoder, &mut file, MAX_FILE_BYTES.min(available))?;
                        file.sync_all()?;
                        staged_bytes += (count as u64).div_ceil(4096) * 4096 + 4096;
                        names.push(key);
                    } else {
                        copy_bounded(&mut decoder, &mut io::sink(), MAX_FILE_BYTES)?;
                    }
                }
            }
        }
        json.end_object().map_err(invalid)?;
        json.consume_trailing_whitespace().map_err(invalid)?;
        if received != length {
            return Err(invalid("Backup upload was incomplete"));
        }
        if version != Some(1) || declared_count.is_some_and(|count| count != file_count) {
            return Err(invalid("Unrecognized backup version or file count"));
        }
        let target = match mode {
            "replace" => Some(runtime.node_id.as_str()),
            "original" => Some(chipid.as_str()),
            _ => None,
        };
        if let Some(target) = target {
            if !matches!(target.len(), 6 | 12) || !target.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid(
                    "Controller ID must be 6 or 12 hexadecimal characters",
                ));
            }
        }
        if web {
            if !includes_web || names.is_empty() {
                return Err(invalid("Backup contains no web files"));
            }
            if store
                .list()?
                .iter()
                .filter(|entry| entry.name.ends_with(".json"))
                .count()
                + names.len()
                > MAX_FILES
            {
                return Err(invalid("File count limit exceeded"));
            }
            if mode != "keep" {
                return Err(invalid(
                    "Web-only restore must use keep mode to preserve binary/gzip assets",
                ));
            }
            commit_web(&store, &names)?;
        } else {
            let bytes = config_bytes.ok_or_else(|| invalid("This backup has no Rust configuration. Import C++ settings with import-config.cmd first."))?;
            let mut candidate = Config::parse(&bytes).map_err(invalid)?;
            if let Some(target) = target {
                rewrite_config(&mut candidate, target);
            }
            candidate.validate().map_err(invalid)?;
            clean_stage(&store)?;
            let mut current = runtime.config.lock().unwrap();
            if runtime.restart_at.load(Ordering::Acquire) != 0 {
                return Err(invalid("Restart already pending"));
            }
            persist(&candidate, &current).map_err(io::Error::other)?;
            *current = candidate;
        }
        runtime
            .restart_at
            .store((runtime.now_ms() / 1000) as u32 + 3, Ordering::Release);
        Ok(if web { names.len() } else { 1 })
    })();
    if !store.root().join(".restore-journal.json").exists() {
        let _ = clean_stage(&store);
    }
    result
}

pub fn reset<F>(runtime: &Arc<Runtime>, mut persist: F) -> Reply
where
    F: FnMut(&Config, &Config) -> Result<(), String>,
{
    let mut current = runtime.config.lock().unwrap();
    if runtime.restart_at.load(Ordering::Acquire) != 0 {
        return Reply::error(409, "Restart already pending");
    }
    let mut candidate = match Config::factory_defaults(&runtime.node_id) {
        Ok(config) => config,
        Err(error) => return Reply::error(500, error),
    };
    candidate.timers = current.timers.clone();
    candidate.network.ssid = runtime.factory_wifi.0.clone();
    candidate.network.wifi_password = runtime.factory_wifi.1.clone();
    candidate.automation = current.automation.clone();
    candidate.management = current.management.clone();
    // Retain schedules and login, but require explicit re-enabling of network outputs.
    for timer in &mut candidate.timers {
        timer.enabled = false;
    }
    for rule in &mut candidate.automation {
        rule.enabled = false;
    }
    if let Err(error) = candidate
        .validate()
        .and_then(|()| persist(&candidate, &current))
    {
        return Reply::error(500, error);
    }
    *current = candidate;
    runtime
        .restart_at
        .store((runtime.now_ms() / 1000) as u32 + 3, Ordering::Release);
    Reply::json(
        200,
        serde_json::json!({"ok":true,"message":"Defaults saved; timers and rules retained but disabled. Restarting with relay OFF.","restart":true}),
    )
}
