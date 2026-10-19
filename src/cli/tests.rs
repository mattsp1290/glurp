use super::main_from;
use std::process::{Command, Output};

const CHILD_CASE: &str = "GLURP_CLI_ADAPTER_RETURN_CASE";
const RETURN_MARKER: &str = "GLURP_CLI_ADAPTER_RETURNED";

fn adapter_child(case: &str) -> Output {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli::tests::adapter_child_returns_to_caller",
            "--nocapture",
        ])
        .env(CHILD_CASE, case)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "adapter child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

#[test]
fn adapter_child_returns_to_caller() {
    let Ok(case) = std::env::var(CHILD_CASE) else {
        return;
    };
    let (args, expected) = match case.as_str() {
        "help" => (["glurp", "--help"], 0),
        "version" => (["glurp", "--version"], 0),
        "invalid" => (["glurp", "--not-a-glurp-option"], 2),
        _ => panic!("unknown adapter case"),
    };
    let code = main_from(args);
    assert_eq!(code, expected);
    // A subprocess exit inside the adapter skips this marker, even for code 0.
    println!("{RETURN_MARKER}={code}");
}

#[test]
fn help_adapter_returns_zero_and_prints_help() {
    let output = adapter_child("help");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Collect private raw chat archives over OpenSSH"));
    assert!(stdout.contains("Usage: glurp <COMMAND>"));
    assert!(stdout.contains(&format!("{RETURN_MARKER}=0")));
    assert!(output.stderr.is_empty());
}

#[test]
fn version_adapter_returns_zero_and_prints_version() {
    let output = adapter_child("version");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(concat!("glurp ", env!("GLURP_VERSION"))));
    assert!(stdout.contains(&format!("{RETURN_MARKER}=0")));
    assert!(output.stderr.is_empty());
}

#[test]
fn invalid_arguments_adapter_returns_two_and_prints_diagnostic() {
    let output = adapter_child("invalid");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains(&format!("{RETURN_MARKER}=2")));
    assert!(stderr.contains("unexpected argument '--not-a-glurp-option' found"));
    assert!(stderr.contains("Usage: glurp <COMMAND>"));
}
