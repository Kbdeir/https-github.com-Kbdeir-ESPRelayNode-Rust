use anyhow::Result;
use node_core::{command::Outcome, health::Service, runtime::Runtime};
use rmodbus::{client::ModbusRequest, ErrorKind, ModbusProto};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

struct Connection {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    frame_started: Instant,
    last_active: Instant,
}
pub fn start(runtime: Arc<Runtime>, revision: u32) -> Result<Vec<thread::JoinHandle<()>>> {
    let config = runtime.config.lock().unwrap().modbus.clone();
    let mut tasks = Vec::new();
    if config.enabled {
        runtime
            .health
            .expect(Service::ModbusServer, (runtime.now_ms() / 1000) as u32);
        let server = runtime.clone();
        tasks.push(
            thread::Builder::new()
                .name("modbus-server".into())
                .stack_size(8192)
                .spawn(move || {
                    let guard = server
                        .health
                        .watch(Service::ModbusServer, (server.now_ms() / 1000) as u32);
                    match serve(server.clone(), revision) {
                        Ok(()) => guard.finish(),
                        Err(e) => log::error!("Modbus server stopped: {e}"),
                    }
                })?,
        );
    }
    let needs_client = {
        let config = runtime.config.lock().unwrap();
        !config.modbus.peers.is_empty()
            || config
                .remote_sensors
                .iter()
                .any(|s| s.transport == node_core::automation::Transport::Modbus && s.configured())
            || config
                .automation
                .iter()
                .any(|r| r.enabled && r.modbus.is_some())
    };
    if needs_client {
        runtime
            .health
            .expect(Service::ModbusClient, (runtime.now_ms() / 1000) as u32);
        let client = runtime.clone();
        tasks.push(
            thread::Builder::new()
                .name("modbus-client".into())
                .stack_size(8192)
                .spawn(move || {
                    let guard = client
                        .health
                        .watch(Service::ModbusClient, (client.now_ms() / 1000) as u32);
                    poll(client.clone(), revision);
                    guard.finish();
                })?,
        );
    }
    Ok(tasks)
}
fn serve(runtime: Arc<Runtime>, revision: u32) -> Result<()> {
    let config = runtime.config.lock().unwrap().modbus.clone();
    let listener = TcpListener::bind(("0.0.0.0", config.port))?;
    listener.set_nonblocking(true)?;
    log::info!(
        "Modbus TCP server listening on port {}; unit {}; maximum four clients",
        config.port,
        config.unit
    );
    let mut clients: Vec<Connection> = Vec::with_capacity(4);
    while runtime.modbus_revision.load(Ordering::Acquire) == revision {
        if let Ok((stream, address)) = listener.accept() {
            if clients.len() < 4
                && config
                    .allowed_clients
                    .iter()
                    .any(|ip| ip == "*" || ip == &address.ip().to_string())
            {
                stream.set_nonblocking(true)?;
                stream.set_nodelay(true)?;
                clients.push(Connection {
                    stream,
                    input: Vec::with_capacity(260),
                    output: Vec::new(),
                    written: 0,
                    frame_started: Instant::now(),
                    last_active: Instant::now(),
                });
            }
        }
        clients.retain_mut(|client| exchange(client, &runtime, config.unit).unwrap_or(false));
        runtime
            .health
            .progress(Service::ModbusServer, (runtime.now_ms() / 1000) as u32);
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn exchange(client: &mut Connection, runtime: &Arc<Runtime>, unit: u8) -> Result<bool> {
    if client.last_active.elapsed() > Duration::from_secs(30)
        || (!client.input.is_empty() && client.frame_started.elapsed() > Duration::from_secs(1))
    {
        return Ok(false);
    }
    if !client.output.is_empty() {
        match client.stream.write(&client.output[client.written..]) {
            Ok(0) => return Ok(false),
            Ok(n) => {
                client.written += n;
                if client.written == client.output.len() {
                    client.output.clear();
                    client.written = 0;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Ok(false),
        }
        return Ok(true);
    }
    let mut buffer = [0u8; 260];
    let capacity = 260 - client.input.len();
    if capacity == 0 {
        return Ok(false);
    }
    match client.stream.read(&mut buffer[..capacity]) {
        Ok(0) => return Ok(false),
        Ok(n) => {
            if client.input.is_empty() {
                client.frame_started = Instant::now();
            }
            client.input.extend_from_slice(&buffer[..n]);
            client.last_active = Instant::now();
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(_) => return Ok(false),
    }
    if client.input.len() >= 7 {
        let length = node_core::modbus::frame_length(&client.input).map_err(anyhow::Error::msg)?;
        if client.input.len() >= length {
            let state = runtime.read();
            client.output =
                node_core::modbus::respond(&client.input[..length], unit, &state, |command| {
                    match runtime.submit(command) {
                        Ok(Outcome::Applied) => Ok(()),
                        Ok(Outcome::InvalidTimer) => Err(ErrorKind::IllegalDataValue),
                        Ok(_) => Err(ErrorKind::SlaveDeviceFailure),
                        Err(_) => Err(ErrorKind::SlaveDeviceBusy),
                    }
                })
                .map_err(anyhow::Error::msg)?;
            client.input.drain(..length);
            client.frame_started = Instant::now();
        }
    }
    Ok(true)
}
fn transact(
    runtime: &Runtime,
    host: &str,
    port: u16,
    bytes: &[u8],
    before_send: impl FnOnce() -> bool,
) -> Result<Vec<u8>> {
    let wifi = runtime.wifi_status.lock().unwrap();
    if !wifi["connected"].as_bool().unwrap_or(false)
        || wifi["ip"].as_str() == Some(host)
        || wifi["setup_ip"].as_str() == Some(host)
    {
        return Err(anyhow::anyhow!("peer offline or destination is this board"));
    }
    drop(wifi);
    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    if !before_send() {
        return Err(anyhow::anyhow!("rule write cancelled"));
    }
    stream.write_all(bytes)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut read_exact = |buffer: &mut [u8]| -> Result<()> {
        let mut read = 0;
        while read < buffer.len() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow::anyhow!("peer deadline"))?;
            stream.set_read_timeout(Some(remaining))?;
            let n = stream.read(&mut buffer[read..])?;
            if n == 0 {
                return Err(anyhow::anyhow!("peer closed"));
            }
            read += n;
        }
        Ok(())
    };
    let mut header = [0u8; 7];
    read_exact(&mut header)?;
    let length = node_core::modbus::frame_length(&header).map_err(anyhow::Error::msg)?;
    let mut response = vec![0u8; length];
    response[..7].copy_from_slice(&header);
    read_exact(&mut response[7..])?;
    Ok(response)
}
fn permitted(runtime: &Runtime, index: usize, revision: u32) -> bool {
    use std::sync::atomic::Ordering;
    runtime.restart_at.load(Ordering::Acquire) == 0
        && runtime.write_revision[index].load(Ordering::Acquire) == revision
        && u64::from(runtime.write_permit_until[index].load(Ordering::Acquire))
            > runtime.now_ms() / 1000
}
fn poll(runtime: Arc<Runtime>, revision: u32) {
    use node_core::automation::{RemoteModbus, RemoteReading, Transport};
    let (peers, sensors, targets) = {
        let config = runtime.config.lock().unwrap();
        (
            config.modbus.peers.clone(),
            config.remote_sensors.to_vec(),
            config
                .automation
                .iter()
                .map(|r| r.modbus.clone())
                .collect::<Vec<_>>(),
        )
    };
    let mut next = vec![0u64; peers.len() + 8];
    let mut readings = vec![serde_json::json!({"valid":false,"stale":true}); peers.len()];
    let mut transaction = 0u16;
    let mut poll_cursor = 0;
    let mut write_cursor = 0;
    while runtime.modbus_revision.load(Ordering::Acquire) == revision {
        let now = runtime.now_ms();
        let writes = *runtime.remote_writes.lock().unwrap();
        for offset in 0..4 {
            let index = (write_cursor + offset) % 4;
            let item = writes[index];
            if !item.pending || item.due_ms > now || !permitted(&runtime, index, item.revision) {
                continue;
            }
            let Some(target) = &targets[index] else {
                continue;
            };
            write_cursor = (index + 1) % 4;
            transaction = transaction.wrapping_add(1);
            let result = (|| -> Result<()> {
                let mut request = ModbusRequest::new(target.unit, ModbusProto::TcpUdp);
                request.tr_id = transaction;
                let mut bytes = Vec::with_capacity(12);
                request.generate_set_coil(target.address, u8::from(item.on), &mut bytes)?;
                let response = transact(&runtime, &target.host, target.port, &bytes, || {
                    runtime.modbus_revision.load(Ordering::Acquire) == revision
                        && permitted(&runtime, index, item.revision)
                })?;
                request.parse_ok(&response)?;
                if response != bytes {
                    return Err(anyhow::anyhow!("coil acknowledgement mismatch"));
                }
                Ok(())
            })();
            let mut current = runtime.remote_writes.lock().unwrap();
            if current[index].revision == item.revision && permitted(&runtime, index, item.revision)
            {
                current[index].failed = result.is_err();
                if result.is_ok() {
                    current[index].pending = false;
                    current[index].acknowledged = Some(item.on);
                } else {
                    current[index].due_ms = runtime.now_ms() + 5000;
                }
            }
            break;
        }
        let total = next.len();
        for offset in 0..total {
            let index = (poll_cursor + offset) % total;
            if next[index] > now {
                continue;
            }
            let (settings, legacy) = if index < peers.len() {
                let peer = &peers[index];
                (
                    RemoteModbus {
                        host: peer.host.clone(),
                        port: peer.port,
                        unit: peer.unit,
                        function: 1,
                        address: peer.coil,
                        value_type: node_core::automation::ValueType::Bool,
                        ..Default::default()
                    },
                    true,
                )
            } else {
                let sensor = &sensors[index - peers.len()];
                if sensor.transport != Transport::Modbus || !sensor.configured() {
                    continue;
                }
                (sensor.modbus.clone(), false)
            };
            poll_cursor = (index + 1) % total;
            next[index] = now
                + if legacy {
                    u64::from(peers[index].poll_ms)
                } else {
                    u64::from(settings.poll) * 1000
                };
            transaction = transaction.wrapping_add(1);
            let result = (|| -> Result<f32> {
                let mut request = ModbusRequest::new(settings.unit, ModbusProto::TcpUdp);
                request.tr_id = transaction;
                let mut bytes = Vec::with_capacity(12);
                match settings.function {
                    1 => request.generate_get_coils(settings.address, 1, &mut bytes)?,
                    2 => request.generate_get_discretes(settings.address, 1, &mut bytes)?,
                    3 => request.generate_get_holdings(
                        settings.address,
                        settings.count(),
                        &mut bytes,
                    )?,
                    4 => request.generate_get_inputs(
                        settings.address,
                        settings.count(),
                        &mut bytes,
                    )?,
                    _ => return Err(anyhow::anyhow!("unsupported remote function")),
                }
                let response = transact(&runtime, &settings.host, settings.port, &bytes, || {
                    runtime.restart_at.load(Ordering::Acquire) == 0
                        && runtime.modbus_revision.load(Ordering::Acquire) == revision
                })?;
                if settings.function <= 2 {
                    let mut bits = Vec::with_capacity(1);
                    request.parse_bool(&response, &mut bits)?;
                    settings
                        .scaled(u8::from(
                            *bits
                                .first()
                                .ok_or_else(|| anyhow::anyhow!("empty bit response"))?,
                        ) as f32)
                        .map_err(anyhow::Error::msg)
                } else {
                    let mut words = Vec::with_capacity(2);
                    request.parse_u16(&response, &mut words)?;
                    settings.decode(&words).map_err(anyhow::Error::msg)
                }
            })();
            if runtime.modbus_revision.load(Ordering::Acquire) != revision {
                return;
            }
            if legacy {
                readings[index] = match result {
                    Ok(value) => {
                        serde_json::json!({"peer":index,"host":settings.host,"coil":settings.address,"on":value!=0.0,"valid":true,"stale":false,"updated_ms":runtime.now_ms()})
                    }
                    Err(_) => {
                        serde_json::json!({"peer":index,"host":settings.host,"valid":false,"stale":true,"error":"peer transaction failed"})
                    }
                };
                *runtime.peer_status.lock().unwrap() = readings.clone();
            } else if let Ok(value) = result {
                let mut values = runtime.remote_values.lock().unwrap();
                if runtime.modbus_revision.load(Ordering::Acquire) == revision {
                    values[index - peers.len()] = RemoteReading {
                        value,
                        seen_ms: Some(runtime.now_ms()),
                    };
                }
            }
            break;
        }
        runtime
            .health
            .progress(Service::ModbusClient, (runtime.now_ms() / 1000) as u32);
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_core::config::Config;
    use std::sync::atomic::Ordering;
    fn runtime() -> Arc<Runtime> {
        let (r, _) = Runtime::new(Config::default(), "test".into(), "test".into());
        *r.wifi_status.lock().unwrap() =
            serde_json::json!({"connected":true,"ip":"192.168.1.100","setup_ip":"192.168.71.1"});
        r
    }
    #[test]
    fn fragmented_register_response_uses_real_tcp_adapter() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 12];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(request[7], 3);
            let reply = [0, 7, 0, 0, 0, 7, 1, 3, 4, 0, 1, 0, 2];
            for chunk in reply.chunks(2) {
                stream.write_all(chunk).unwrap();
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut request = ModbusRequest::new(1, ModbusProto::TcpUdp);
        request.tr_id = 7;
        let mut bytes = Vec::new();
        request.generate_get_holdings(0, 2, &mut bytes).unwrap();
        let response = transact(&runtime(), "127.0.0.1", port, &bytes, || true).unwrap();
        let mut words = Vec::new();
        request.parse_u16(&response, &mut words).unwrap();
        assert_eq!(words, [1, 2]);
        server.join().unwrap();
    }
    #[test]
    fn slow_partial_frame_has_whole_response_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 12];
            stream.read_exact(&mut request).unwrap();
            for byte in [0, 1, 0, 0, 0, 6, 1] {
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(350));
            }
        });
        let start = Instant::now();
        assert!(transact(&runtime(), "127.0.0.1", port, &[0; 12], || true).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
        server.join().unwrap();
    }
    #[test]
    fn cancellation_prevents_any_request_bytes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        });
        assert!(transact(&runtime(), "127.0.0.1", port, &[0; 12], || false).is_err());
        server.join().unwrap();
    }
    #[test]
    fn revision_restart_and_lease_cancel_queued_writes() {
        let r = runtime();
        assert!(!permitted(&r, 0, 0));
        r.write_permit_until[0].store(3, Ordering::Release);
        assert!(permitted(&r, 0, 0));
        r.write_revision[0].store(1, Ordering::Release);
        assert!(!permitted(&r, 0, 0));
        assert!(permitted(&r, 0, 1));
        r.restart_at.store(1, Ordering::Release);
        assert!(!permitted(&r, 0, 1));
    }
    #[test]
    fn self_addresses_cannot_form_actuation_feedback_loops() {
        for host in ["192.168.1.100", "192.168.71.1"] {
            assert!(transact(&runtime(), host, 502, &[0; 12], || true).is_err());
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn connect(port: u16) -> TcpStream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                return stream;
            }
            assert!(Instant::now() < deadline, "Modbus listener did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn join_workers(tasks: Vec<thread::JoinHandle<()>>) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !tasks.iter().all(|t| t.is_finished()) {
            assert!(Instant::now() < deadline, "Modbus worker did not retire");
            thread::sleep(Duration::from_millis(10));
        }
        for task in tasks {
            task.join().unwrap();
        }
    }
    fn read_off(stream: &mut TcpStream, unit: u8) {
        stream
            .write_all(&[0, 1, 0, 0, 0, 6, unit, 1, 0, 0, 0, 1])
            .unwrap();
        let mut response = [0; 10];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(response, [0, 1, 0, 0, 0, 4, unit, 1, 1, 0]);
    }
    fn change(runtime: &Runtime, update: impl FnOnce(&mut node_core::config::Config)) -> u32 {
        let mut current = runtime.config.lock().unwrap();
        let mut updated = current.clone();
        update(&mut updated);
        runtime.config_saved(&updated, &current);
        *current = updated;
        runtime.modbus_revision.load(Ordering::Acquire)
    }

    #[test]
    fn live_modbus_port_unit_allowlist_and_disable_reenable_release_workers() {
        let r = runtime();
        let first = free_port();
        change(&r, |c| {
            c.modbus.enabled = true;
            c.modbus.port = first;
            c.modbus.allowed_clients = vec!["127.0.0.1".into()];
        });
        let tasks = start(r.clone(), r.modbus_revision.load(Ordering::Acquire)).unwrap();
        let mut old = connect(first);
        read_off(&mut old, 1);
        let second = free_port();
        let revision = change(&r, |c| {
            c.modbus.port = second;
            c.modbus.unit = 7;
        });
        join_workers(tasks);
        assert!(TcpStream::connect(("127.0.0.1", first)).is_err());
        let mut buffer = [0; 1];
        assert!(matches!(old.read(&mut buffer), Ok(0) | Err(_)));
        let tasks = start(r.clone(), revision).unwrap();
        read_off(&mut connect(second), 7);
        let revision = change(&r, |c| {
            c.modbus.allowed_clients = vec!["192.168.1.10".into()]
        });
        join_workers(tasks);
        let tasks = start(r.clone(), revision).unwrap();
        assert!(matches!(connect(second).read(&mut buffer), Ok(0) | Err(_)));
        let revision = change(&r, |c| c.modbus.enabled = false);
        join_workers(tasks);
        assert!(start(r.clone(), revision).unwrap().is_empty());
        for _ in 0..10 {
            let revision = change(&r, |c| {
                c.modbus.enabled = true;
                c.modbus.allowed_clients = vec!["*".into()];
            });
            let tasks = start(r.clone(), revision).unwrap();
            read_off(&mut connect(second), 7);
            change(&r, |c| c.modbus.enabled = false);
            join_workers(tasks);
            assert_eq!(r.health.fault((r.now_ms() / 1000) as u32), None);
            assert_eq!(r.restart_at.load(Ordering::Acquire), 0);
        }
    }
}
