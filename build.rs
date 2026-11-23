//! Deploys bundled assets and Mods and installs initial configuration beside the launcher.

use std::{fs, io, path::Path};

const ASSETS_DIR: &str = "assets";

fn main() {
    let workspace_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let assets = workspace_dir.join(ASSETS_DIR);
    let mods = workspace_dir.join("mods");
    let config = workspace_dir.join("config");
    for source in [&assets, &mods, &config] {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is unavailable");
    // In the workspace's Cargo layout, intermediate build output is under
    // the executable's profile directory, in `build/<package output>/out`.
    let profile = Path::new(&out_dir)
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
        .expect("Cargo profile output directory is unavailable");

    deploy_assets(&assets, &profile.join(ASSETS_DIR))
        .expect("failed to deploy assets beside the launcher executable");
    copy_mods(&mods, &profile.join("mods"))
        .expect("failed to deploy Mods beside the launcher executable");
    install_config(&config, &profile.join("config"))
        .expect("failed to install categorized configuration beside the launcher executable");
}

fn deploy_assets(source: &Path, destination: &Path) -> io::Result<()> {
    let staging = destination.with_file_name(".roundo-assets.deploy");
    fs::create_dir(&staging)?;
    // Copy first so a failed copy leaves the previous assets intact.
    let result = copy_directory_contents(source, &staging).and_then(|()| {
        match fs::remove_dir_all(destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&staging, destination)
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_mods(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_mods(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

// Never overwrite installed configuration, especially its ID checkpoint.
fn install_config(source: &Path, destination: &Path) -> io::Result<()> {
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
    let result =
        copy_directory_contents(source, &staging).and_then(|()| fs::rename(&staging, destination));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_directory_contents(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir(&target)?;
            copy_directory_contents(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(io::Error::other(format!(
                "unsupported bundled file entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}
