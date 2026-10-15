#[path = "../build_inputs.rs"]
mod build_inputs;
#[test]
fn fixture_inputs_include_transitive_sources_and_build_configuration() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join("src"))?;
    for path in [
        "src/project.rs",
        "src/evidence.rs",
        "src/state.rs",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "build_inputs.rs",
        "rust-toolchain.toml",
    ] {
        std::fs::write(root.path().join(path), "initial")?;
    }
    let initial = build_inputs::collect(root.path())?;
    assert_eq!(initial, build_inputs::collect(root.path())?);
    for path in [
        "src/project.rs",
        "src/evidence.rs",
        "src/state.rs",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "build_inputs.rs",
        "rust-toolchain.toml",
        "src/new_module.rs",
    ] {
        let path = root.path().join(path);
        std::fs::write(&path, "changed")?;
        assert_ne!(initial, build_inputs::collect(root.path())?);
        if path.ends_with("new_module.rs") {
            std::fs::remove_file(path)?;
        } else {
            std::fs::write(path, "initial")?;
        }
    }
    Ok(())
}
