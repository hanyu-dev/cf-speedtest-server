//! Get git infos

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::{env, io};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../crates");

    let version = env!("CARGO_PKG_VERSION");

    let source =
        env::var("CF_SPEEDTEST_SERVER_CORE_BUILD_SOURCE").unwrap_or_else(|_| "commit".to_string());

    let commit = env::var("CF_SPEEDTEST_SERVER_CORE_BUILD_COMMIT")
        .or_else(|_| run("git", &["describe", "--always"]))
        .unwrap_or_else(|_| "unknown".to_string());

    let build = if cfg!(debug_assertions) || cfg!(test) {
        "debug"
    } else {
        "release"
    };

    let version = format!("{version}+{source}.{commit}.{build}").replace('\n', "");

    File::create(Path::new(&env::var("OUT_DIR")?).join("VERSION"))?
        .write_all(version.trim().as_bytes())?;

    Ok(())
}

fn run(cmd: &str, args: &[&str]) -> io::Result<String> {
    let output = Command::new(cmd).args(args).output()?;

    if output.status.success() {
        String::from_utf8(output.stdout).map_err(io::Error::other)
    } else {
        Err(io::Error::other(format!(
            "Command `{cmd} {args:?}` failed with error: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}
