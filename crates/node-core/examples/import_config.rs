use node_core::config::Config;
use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if !(2..=4).contains(&args.len()) {
        return Err("usage: import_config <reference-project> <output-config.local.json> [--enable-network] [--allow-any-modbus]".into());
    }
    let reference = PathBuf::from(&args[0]).join("data");
    let read = |name: &str| -> Result<Value, Box<dyn std::error::Error>> {
        Ok(serde_json::from_slice(&fs::read(reference.join(name))?)?)
    };
    let mut config = Config::from_legacy(
        &read("config.json")?,
        &read("relay0.json")?,
        &read("IRMAP.json")?,
    )?;
    for flag in args.iter().skip(2) {
        match flag.to_str() {
            Some("--enable-network") => {
                config.management.enabled = true;
                let mut random = [0u8; 16];
                getrandom::getrandom(&mut random)
                    .map_err(|_| "could not generate setup credentials")?;
                config.management.password = random.iter().map(|v| format!("{v:02x}")).collect();
            }
            Some("--allow-any-modbus") => {
                config.modbus.enabled = true;
                config.modbus.allowed_clients = vec!["*".into()];
            }
            _ => return Err("unknown import flag".into()),
        }
    }
    // Do not activate inherited schedules automatically while bringing up network services.
    config.validate()?;
    let destination = PathBuf::from(&args[1]);
    if destination.exists() {
        return Err("destination exists; review it before creating another import".into());
    }
    let destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    serde_json::to_writer_pretty(destination, &config)?;
    println!("Imported single-relay settings. Credentials were not printed.");
    println!("Network activation is explicit. Web credentials are in management.username/password in the private output file.");
    println!("Sensors, automation rules, schedules, VPN, and persisted relay state require later migration stages.");
    Ok(())
}
