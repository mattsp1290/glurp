#[test]
fn standalone_public_api_works_without_installed_cli() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_glurp-library-consumer"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("no glurp executable on PATH"));
}
