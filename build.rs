mod build_inputs;
fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    for path in [
        "src",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "build_inputs.rs",
        "rust-toolchain.toml",
        ".cargo",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let mut bytes = build_inputs::collect(&root).expect("Fixture source inputs");
    let mut context: Vec<_> = std::env::vars()
        .filter(|(key, _)| {
            key.starts_with("CARGO_FEATURE_")
                || key.starts_with("CARGO_CFG_")
                || matches!(
                    key.as_str(),
                    "TARGET" | "PROFILE" | "OPT_LEVEL" | "DEBUG" | "CARGO_ENCODED_RUSTFLAGS"
                )
        })
        .collect();
    context.sort();
    bytes.extend_from_slice(format!("{context:?}").as_bytes());
    let compiler = std::process::Command::new(std::env::var_os("RUSTC").unwrap())
        .arg("--version")
        .output()
        .expect("Compiler version");
    assert!(compiler.status.success());
    bytes.extend_from_slice(&compiler.stdout);
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
            .join("capture-fixture-inputs"),
        bytes,
    )
    .expect("Fixture input artifact");
}
