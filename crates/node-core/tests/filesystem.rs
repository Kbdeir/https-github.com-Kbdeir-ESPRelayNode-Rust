use node_core::{
    config::Config,
    filesystem::{self, FileStore},
    management,
    runtime::Runtime,
    web,
};
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
static NEXT: AtomicU32 = AtomicU32::new(0);
struct Fixture {
    root: PathBuf,
    runtime: Arc<Runtime>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rust-files-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let (runtime, _) = Runtime::new(Config::default(), "test".into(), "test".into());
        *runtime.files.lock().unwrap() = Some(Arc::new(FileStore::new(&root, None)));
        Self { root, runtime }
    }
    fn request(&self, method: &str, uri: &str, body: &[u8]) -> web::Reply {
        web::handle(&self.runtime, method, uri, body, |_, _| {
            panic!("File edits cannot alter NVS")
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
#[test]
fn paths_are_flat_bounded_and_cannot_escape_or_access_hidden_files() {
    for name in [
        "/../config.local.json",
        "/foo/../bar",
        "/foo/bar",
        "/.upload.tmp",
        "config.html",
        "/a\\b",
        "/a\0b",
        "/a%2fb",
        "/",
    ] {
        assert!(!filesystem::valid_path(name), "{name}");
    }
    assert!(!filesystem::valid_path(&format!("/{}", "x".repeat(64))));
    assert!(filesystem::valid_path("/Automation.html.gz"));
}
#[test]
fn list_upload_edit_download_rename_delete_use_actual_files() {
    let f = Fixture::new();
    assert_eq!(
        f.request("POST", "/api/files/content?path=%2Ftest.txt", b"first")
            .status,
        200
    );
    assert_eq!(
        f.request("GET", "/api/files/content?path=%2Ftest.txt", b"")
            .body
            .into_bytes(),
        b"first"
    );
    assert_eq!(
        f.request("POST", "/api/files/content?path=%2Ftest.txt", b"second")
            .status,
        200
    );
    let listed: serde_json::Value =
        serde_json::from_slice(&f.request("GET", "/api/files/list", b"").body.into_bytes())
            .unwrap();
    assert_eq!(listed["files"][0]["name"], "/test.txt");
    assert_eq!(listed["files"][0]["size"], 6);
    assert_eq!(
        f.request("GET", "/api/files/delete?path=%2Ftest.txt", b"")
            .status,
        405
    );
    assert_eq!(
        f.request(
            "POST",
            "/api/files/rename",
            b"from=%2Ftest.txt&to=%2Frenamed.txt"
        )
        .status,
        200
    );
    assert_eq!(
        f.request("GET", "/api/files/content?path=%2Ftest.txt", b"")
            .status,
        404
    );
    assert_eq!(
        f.request("POST", "/api/files/delete", b"path=%2Frenamed.txt")
            .status,
        200
    );
    assert_eq!(f.runtime.restart_at.load(Ordering::Relaxed), 0);
}
#[test]
fn interrupted_upload_preserves_old_file_and_removes_temporary_file() {
    let f = Fixture::new();
    fs::write(f.root.join("test.txt"), b"original").unwrap();
    let reply = web::upload_file(
        &f.runtime,
        "/api/files/content?path=%2Ftest.txt",
        100,
        b"short".as_slice(),
    );
    assert_ne!(reply.status, 200);
    assert_eq!(fs::read(f.root.join("test.txt")).unwrap(), b"original");
    assert!(!f.root.join(".upload.tmp").exists());
}
#[test]
fn large_upload_is_streamed_and_bounds_are_enforced() {
    let f = Fixture::new();
    let bytes = vec![b'x'; 100_000];
    assert_eq!(
        web::upload_file(
            &f.runtime,
            "/api/files/content?path=%2Flarge.txt",
            bytes.len(),
            bytes.as_slice()
        )
        .status,
        200
    );
    assert_eq!(
        f.request("GET", "/api/files/content?path=%2Flarge.txt", b"")
            .body
            .into_bytes(),
        bytes
    );
    assert_eq!(
        web::upload_file(
            &f.runtime,
            "/api/files/content?path=%2Flarge.txt",
            filesystem::MAX_FILE_BYTES + 1,
            io::empty()
        )
        .status,
        413
    );
    assert_eq!(
        f.request("POST", "/api/files/content?path=%2F..%2Fsecret", b"bad")
            .status,
        400
    );
    fs::write(f.root.join("exists.txt"), b"keep").unwrap();
    assert_eq!(
        f.request(
            "POST",
            "/api/files/rename",
            b"from=%2Flarge.txt&to=%2Fexists.txt"
        )
        .status,
        409
    );
}
#[test]
fn page_edits_are_served_immediately_and_gzip_is_raw_with_correct_headers() {
    let f = Fixture::new();
    fs::write(
        f.root.join("config.html"),
        b"<html>%RSTATE% %UNKNOWN%</html>",
    )
    .unwrap();
    assert!(matches!(
        f.request("GET", "/", b"").body,
        web::Body::Asset(_)
    ));
    assert_eq!(
        f.request("GET", "/", b"").body.into_bytes(),
        b"<html>OFF %UNKNOWN%</html>"
    );
    fs::write(f.root.join("config.html"), b"edited").unwrap();
    assert_eq!(f.request("GET", "/", b"").body.into_bytes(), b"edited");
    fs::remove_file(f.root.join("config.html")).unwrap();
    fs::write(f.root.join("config.html.gz"), [0x1f, 0x8b, 0x08]).unwrap();
    let reply = f.request("GET", "/", b"");
    assert_eq!(reply.headers, [("Content-Encoding", "gzip".into())]);
    assert_eq!(reply.mime, "text/html; charset=utf-8");
    assert_eq!(reply.body.into_bytes(), [0x1f, 0x8b, 0x08]);
    assert!(f
        .request("GET", "/api/files/content?path=%2Fconfig.html.gz", b"")
        .headers
        .is_empty());
}
#[test]
fn reader_templates_handle_chunk_boundaries_percentages_and_inserted_tokens() {
    let config = Config::default();
    let context = management::RenderContext {
        config: &config,
        slot: 1,
        state: Default::default(),
        time: "<%RSTATE%>",
        uptime: 0,
        active: false,
    };
    let text = "x".repeat(766) + "%RSTATE% width:50%; %UNKNOWN% %systemtime% %";
    let mut output = Vec::new();
    management::render_reader(text.as_bytes(), &context, |chunk| {
        assert!(chunk.len() <= 768);
        output.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        management::render(
            &text,
            &config,
            1,
            Default::default(),
            "<%RSTATE%>",
            0,
            false
        )
    );
    assert!(
        management::render_reader(io::repeat(b'x').take(4000), &context, |_| Err(
            io::Error::other("closed")
        ))
        .is_err()
    );
}
#[test]
fn unmounted_filesystem_never_silently_serves_embedded_pages() {
    let (r, _) = Runtime::new(Config::default(), "test".into(), "test".into());
    assert_eq!(
        web::handle(&r, "GET", "/", b"", |_, _| panic!()).status,
        503
    );
    assert_eq!(
        web::handle(&r, "GET", "/api/files/list", b"", |_, _| panic!()).status,
        503
    );
    assert_eq!(
        web::handle(&r, "GET", "/api/status", b"", |_, _| panic!()).status,
        200
    );
}

#[test]
fn full_listing_is_streamed_and_file_count_limit_rejects_new_upload() {
    let f = Fixture::new();
    for index in 0..filesystem::MAX_FILES {
        fs::write(f.root.join(format!("{index:03}{}", "x".repeat(60))), b"x").unwrap();
    }
    let reply = f.request("GET", "/api/files/list", b"");
    assert!(matches!(reply.body, web::Body::FileList(_, _, _)));
    let mut bytes = Vec::new();
    reply
        .body
        .write_to(|chunk| {
            assert!(chunk.len() <= 768);
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["files"].as_array().unwrap().len(),
        filesystem::MAX_FILES
    );
    assert_eq!(
        f.request("POST", "/api/files/content?path=%2Fextra.txt", b"extra")
            .status,
        507
    );
}
use std::io::Read;
