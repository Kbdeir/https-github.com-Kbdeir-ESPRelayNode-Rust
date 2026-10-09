use anyhow::Result;
use embedded_svc::http::Headers;
use esp_idf_svc::{
    http::{
        server::{Configuration, EspHttpServer},
        Method,
    },
    io::{Read, Write},
    nvs::EspDefaultNvsPartition,
};
use node_core::{
    runtime::Runtime,
    web::{self, Reply},
};
use std::sync::Arc;

struct RequestReader<'a, R>(&'a mut R);
impl<R: Read> std::io::Read for RequestReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .read(buffer)
            .map_err(|error| std::io::Error::other(format!("{error:?}")))
    }
}

pub fn start(runtime: Arc<Runtime>, nvs: EspDefaultNvsPartition) -> Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        stack_size: 32768,
        max_open_sockets: 4,
        max_uri_handlers: 4,
        uri_match_wildcard: true,
        ..Default::default()
    })?;
    for method in [Method::Get, Method::Post] {
        let runtime = runtime.clone();
        let nvs = nvs.clone();
        server.fn_handler::<anyhow::Error,_>("/*",method,move |mut request| {
            let uri=request.uri().to_owned();
            let authorized=web::authorized(request.header("Authorization"),&runtime.config.lock().unwrap());
            if !authorized {
                request.into_response(401,None,&[("WWW-Authenticate","Basic realm=\"SmartConfig\""),("Cache-Control","no-store")])?.write_all(b"Authentication required")?;return Ok(());
            }
            if method==Method::Post && request.header("X-SmartConfig")!=Some("1") {request.into_status_response(403)?.write_all(b"Same-origin request header required")?;return Ok(());}
            if unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } < 24_576 || unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_8BIT) } < 12_288 {
                request.into_response(503,None,&[("Connection","close"),("Retry-After","2")])?.write_all(b"Low memory; retry shortly")?;return Ok(());
            }
            let path=uri.split('?').next().unwrap_or("");
            if method==Method::Get && matches!(path,"/api/firmware/download"|"/api/filesystem/download") {
                let filesystem=path=="/api/filesystem/download";
                let length=crate::firmware::download_length(filesystem)?;
                let disposition=if filesystem {"attachment; filename=\"littlefs.bin\""} else {"attachment; filename=\"firmware.bin\""};
                let length=length.to_string();
                let mut response=request.into_response(200,None,&[("Content-Type","application/octet-stream"),("Content-Length",&length),("Content-Disposition",disposition),("Cache-Control","no-store"),("Connection","close"),("X-Content-Type-Options","nosniff")])?;
                crate::firmware::download(filesystem,|chunk| response.write_all(chunk).map_err(|error|std::io::Error::other(error.to_string())))?;
                return Ok(());
            }
            let len=request.content_len().unwrap_or(0) as usize;
            let upload = method==Method::Post && path=="/api/files/content";
            let restore = method==Method::Post && path=="/api/restore";
            let image = method==Method::Post && matches!(path,"/update"|"/updatefs");
            let limit = if image || restore {node_core::backup::MAX_BACKUP_BYTES} else if upload { node_core::filesystem::MAX_FILE_BYTES } else { node_core::config::MAX_CONFIG_BYTES };
            if len>limit {request.into_status_response(413)?.write_all(b"Request too large")?;return Ok(());}
            if method==Method::Post && (request.header("Transfer-Encoding").is_some() || request.content_len().is_none()) {request.into_status_response(411)?.write_all(b"Content-Length required")?;return Ok(());}
            let stream=upload||restore||image;
            let mut body=vec![0u8;if stream {0} else {len}]; if !stream && len>0 {request.read_exact(&mut body)?;}
            let reply=if image {
                if request.header("Content-Type")!=Some("application/octet-stream") {Reply::error(415,"Upload the raw application or LittleFS .bin file")} else {
                    let result=if path=="/update" {crate::firmware::upload_firmware(&runtime,len,RequestReader(&mut request))} else {crate::firmware::upload_filesystem(&runtime,&nvs,len,RequestReader(&mut request))};
                    match result {
                        Ok(())=> Reply {status:200,mime:"text/plain",headers:Vec::new(),body:web::Body::Bytes(b"OK - Update validated. Rebooting with relay OFF.".to_vec())},
                        Err(error)=> Reply::error(400,error.to_string()),
                    }
                }
            } else if restore {
                node_core::backup::restore(&runtime,&uri,len,RequestReader(&mut request),|candidate,previous|crate::storage::save(candidate,previous,&nvs).map_err(|e|e.to_string()))
            } else if upload {
                web::upload_file(&runtime, &uri, len, RequestReader(&mut request))
            } else if method==Method::Get && path=="/api/maintenance" {
                crate::firmware::status(&runtime)
            } else if method==Method::Get && path=="/api/heap" {
                Reply::json(200,serde_json::json!({"heap":unsafe{esp_idf_svc::sys::esp_get_free_heap_size()},"heapMin":unsafe{esp_idf_svc::sys::esp_get_minimum_free_heap_size()},"heapMaxBlk":unsafe{esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_8BIT)},"httpStackMargin":unsafe{esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut())}}))
            }else{web::handle(&runtime,if method==Method::Get{"GET"}else{"POST"},&uri,&body,|candidate,previous|crate::storage::save(candidate,previous,&nvs).map_err(|e|e.to_string()))};
            let mut headers = vec![("Content-Type",reply.mime),("Cache-Control","no-store"),("X-Content-Type-Options","nosniff"),("X-Frame-Options","DENY"),("Connection","close")];
            headers.extend(reply.headers.iter().map(|(key,value)| (*key,value.as_str())));
            let mut response = request.into_response(reply.status,None,&headers)?;
            reply.body.write_to(|chunk| response.write_all(chunk).map_err(|error| std::io::Error::other(error.to_string())))?;
            Ok(())
        })?;
    }
    Ok(server)
}
