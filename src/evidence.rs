use crate::domain::{Evidence, hash};
use anyhow::{Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn save(root: &Path, evidence: &Evidence) -> Result<String> {
    let bytes = serde_json::to_vec(evidence)?;
    let digest = hash(&bytes);
    let dir = root.join("evidence");
    fs::create_dir_all(&dir)?;
    File::open(root)?.sync_all()?;
    let target = dir.join(format!("{digest}.json"));
    if target.exists() {
        ensure!(
            hash(&fs::read(&target)?) == digest,
            "Corrupt existing evidence"
        );
        return Ok(digest);
    }
    let temp = dir.join(format!(".{digest}.{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &target)?;
        File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(digest)
}
pub fn load(root: &Path, digest: &str) -> Result<Evidence> {
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid evidence digest"
    );
    let bytes = fs::read(root.join("evidence").join(format!("{digest}.json")))?;
    ensure!(hash(&bytes) == digest, "Evidence hash mismatch");
    Ok(serde_json::from_slice(&bytes)?)
}
/// Bound a local runtime artifact before decoding; API projection is retained separately.
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Artifact exceeds capture limit");
    Ok(bytes)
}
pub fn private_directory(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path.canonicalize()?)
}
