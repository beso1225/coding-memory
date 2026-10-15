//! Deterministic build-time inputs for installed-runtime fixture identity.
use std::{
    fs, io,
    path::{Path, PathBuf},
};
fn sources(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}
pub fn collect(root: &Path) -> io::Result<Vec<u8>> {
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files)?;
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "build_inputs.rs",
        "rust-toolchain.toml",
        ".cargo/config",
        ".cargo/config.toml",
    ] {
        let path = root.join(name);
        if path.exists() {
            files.push(path);
        }
    }
    files.sort();
    let mut bytes = Vec::new();
    for file in files {
        let name = file.strip_prefix(root).unwrap().to_string_lossy();
        let content = fs::read(&file)?;
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&content);
    }
    Ok(bytes)
}
