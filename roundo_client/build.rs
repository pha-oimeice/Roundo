use std::fs;
use std::io;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../mods");

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("../mods");
    let out_dir = Path::new(&std::env::var_os("OUT_DIR").expect("OUT_DIR is unavailable"))
        .ancestors()
        .nth(3)
        .expect("Cargo profile output directory is unavailable")
        .join("mods");

    copy_directory(&source, &out_dir).expect("failed to deploy Mods beside the client executable");
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}
