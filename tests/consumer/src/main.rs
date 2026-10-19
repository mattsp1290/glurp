//! This executable is a real standalone Cargo consumer. It does not shell out
//! to glurp; only its environment-isolation child and the SSH fixture are run.
mod scenarios;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--child") {
        let scenario = std::env::args().nth(2).expect("scenario argument");
        let fixture = scenarios::Fixture::from_environment();
        let runtime = scenarios::runtime(&scenario);
        runtime.block_on(scenarios::run(&scenario, &fixture));
        return;
    }
    for scenario in [
        "smoke",
        "equivalence",
        "roots",
        "independent_cancel",
        "busy",
        "privacy",
    ] {
        let fixture = scenarios::Fixture::create();
        let mut command = fixture.command(std::env::current_exe().unwrap());
        command.args(["--child", scenario]);
        if scenario == "smoke" {
            command.env_remove("HOME");
        }
        let output = fixture.bounded_output(command);
        assert!(
            output.status.success(),
            "consumer scenario {scenario} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "library emitted stdout");
        assert!(output.stderr.is_empty(), "library emitted stderr");
    }
    println!("Standalone public API consumer passed with no glurp executable on PATH.");
}
