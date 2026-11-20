//! Make `memory.x` visible to the linker, and pass the watch's Bluetooth
//! address to the app at compile time.
//!
//! The address is personal to one watch, so it is not in the sources. It
//! comes from the environment variable `OPW_MAC`, or from a line
//! `OPW_MAC=AA:BB:CC:DD:EE:FF` in the untracked file `local.env`
//! (see `local.env.example`). `tools/ota.sh` reads the same file.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=local.env");
    println!("cargo:rerun-if-env-changed=OPW_MAC");

    let mac = env::var("OPW_MAC").ok().or_else(|| {
        let text = fs::read_to_string("local.env").ok()?;
        text.lines()
            .filter_map(|line| line.trim().strip_prefix("OPW_MAC="))
            .map(|value| value.trim().trim_matches('"').to_string())
            .next_back()
    });
    let mac = mac.unwrap_or_else(|| {
        panic!("the watch's Bluetooth address is not set: copy local.env.example to local.env and fill in OPW_MAC")
    });
    let parts: Vec<&str> = mac.split(':').collect();
    let valid = parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && u8::from_str_radix(p, 16).is_ok());
    if !valid {
        panic!("OPW_MAC must look like AA:BB:CC:DD:EE:FF, got {mac:?}");
    }
    println!("cargo:rustc-env=W301_BT_ADDRESS={mac}");
}
