fn main() {
    use std::{env, fs, path::PathBuf};

    let local_config =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("config.local.json");
    println!("cargo:rerun-if-changed={}", local_config.display());
    let config = if local_config.exists() {
        node_core::config::Config::parse(&fs::read(local_config).expect("read config.local.json"))
            .expect("invalid config.local.json")
    } else {
        node_core::config::Config::default()
    };
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("bootstrap.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .expect("write validated bootstrap configuration");
    embuild::espidf::sysenv::output();
}
