//! launchd configuration with explicit paths, argv and a controlled executable search path.
use crate::{state::State, worker::Config};
use anyhow::{Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn string(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 service path"))?;
    ensure!(
        !s.chars().any(char::is_control),
        "Control character in service path"
    );
    Ok(format!("<string>{}</string>", xml(s)))
}
pub fn plist(root: &Path, config_path: &Path, binary: &Path, label: &str) -> Result<String> {
    ensure!(
        root.is_absolute() && config_path.is_absolute() && binary.is_absolute() && binary.is_file(),
        "Service paths must be absolute"
    );
    crate::worker::executable(binary)?;
    ensure!(
        !label.is_empty()
            && label.len() <= 128
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        "Invalid launchd label"
    );
    let root = root.canonicalize()?;
    let config = Config::load(config_path)?;
    config.validate(&root, &State::open(&root.join("state.sqlite"))?)?;
    let path = std::env::join_paths(&config.executable_path)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("Non-UTF8 executable path"))?;
    let argv = [
        string(binary)?,
        "<string>--state-dir</string>".into(),
        string(&root)?,
        "<string>worker</string>".into(),
        "<string>--config</string>".into(),
        string(config_path)?,
    ]
    .join("");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{argv}</array><key>WorkingDirectory</key>{}<key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>30</integer><key>StandardOutPath</key>{}<key>StandardErrorPath</key>{}</dict></plist>\n",
        xml(label),
        string(&root)?,
        xml(&path),
        string(&root.join("worker.stdout.log"))?,
        string(&root.join("worker.stderr.log"))?
    ))
}
pub fn write(path: &Path, content: &str) -> Result<()> {
    ensure!(path.is_absolute(), "Service output path must be absolute");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    File::open(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("No service parent"))?,
    )?
    .sync_all()?;
    Ok(())
}
