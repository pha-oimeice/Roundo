//! Small synchronous filesystem helpers exposed through the legacy async API.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

#[inline(always)]
/// Reports whether a path resolves to a regular file.
pub async fn exists_file(path: &PathBuf) -> bool {
    let open_result = File::open(path);
    open_result.is_ok()
}

#[inline(always)]
/// Returns the executable directory, falling back to the current directory.
pub fn get_exe_root_path() -> PathBuf {
    std::env::current_exe()
        .expect("current executable path must be available")
        .parent()
        .expect("current executable path must have a parent directory")
        .to_path_buf()
}

#[inline(always)]
/// Creates or truncates a file and writes the complete UTF-8 payload.
pub async fn write_all_to_file(file_path: &PathBuf, content: &str) -> io::Result<()> {
    let create_result = File::create(file_path);
    let mut file = create_result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot create file {}: {error}", file_path.display()),
        )
    })?;
    let write_result = file.write_all(content.as_bytes());
    write_result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot write file {}: {error}", file_path.display()),
        )
    })
}

#[inline(always)]
/// Creates a directory tree when it does not already exist.
pub async fn create_all_dirs(path: &PathBuf) -> io::Result<()> {
    let create_result = std::fs::create_dir_all(path);
    create_result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot create directory {}: {error}", path.display()),
        )
    })
}
