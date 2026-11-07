//! Installs the bundled configuration on the first build only. An existing
//! installation is never repaired or overwritten, especially its ID checkpoint.

use std::{fs, io, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=../../../config");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("../../../config");
    let output = std::env::var_os("OUT_DIR").expect("OUT_DIR is unavailable");
    let profile = Path::new(&output)
        .ancestors()
        .nth(3)
        .expect("Cargo profile output directory is unavailable");
    deploy(&source, &profile.join("config"))
        .expect("failed to install categorized configuration beside the executables");
}

fn deploy(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.try_exists()? {
        if !destination.is_dir() {
            return Err(io::Error::other(format!(
                "configuration root is not a directory: {}",
                destination.display()
            )));
        }
        return Ok(());
    }
    let staging = destination.with_file_name(".roundo-config.deploy");
    // A stale staging directory requires inspection rather than automatic reuse.
    fs::create_dir(&staging)?;
    let result = copy_directory(source, &staging).and_then(|()| fs::rename(&staging, destination));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir(&target)?;
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(io::Error::other(format!(
                "unsupported configuration entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}
