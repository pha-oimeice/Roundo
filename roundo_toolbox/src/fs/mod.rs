//! Currently fs does not yet have robust implementation of async io, so simply use std.
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[inline(always)]
pub async fn exists_file(path: &PathBuf) -> bool {
    File::open(path).is_ok()
}

#[inline(always)]
pub fn get_exe_root_path() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[inline(always)]
pub async fn write_all_to_file(file_path: &PathBuf, content: &str) {
    File::create(file_path)
        .unwrap_or_else(|e| {
            panic!("Failed to create file {}: {}", file_path.display(), e);
        })
        .write_all(content.as_bytes())
        .unwrap_or_else(|e| {
            panic!(
                "Failed to write content to file {}: {}",
                file_path.display(),
                e
            );
        });
}

#[inline(always)]
pub async fn create_all_dirs(path: &PathBuf) {
    std::fs::create_dir_all(path).expect(&format!(
        "Failed to create directories for path {}",
        path.display()
    ));
}
