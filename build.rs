use std::env;

fn main() {
    // Worktrees have their own HEAD/index and share branch/tag references.
    let mut paths = vec![
        "HEAD".to_owned(),
        "index".into(),
        "refs/tags".into(),
        "packed-refs".into(),
    ];
    if let Some(branch) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        paths.push(branch);
    }
    for path in paths {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // Dirty versions also change when tracked docs and build policy change.
    if let Some(files) = git_output(&["ls-files", "-z"]) {
        for path in files.split('\0').filter(|path| !path.is_empty()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-env-changed=GLURP_VERSION");
    let version = env::var("GLURP_VERSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            git_output(&["describe", "--tags", "--match", "v*", "--always", "--dirty"])
                .unwrap_or_else(|| "dev".into())
        });
    println!("cargo:rustc-env=GLURP_VERSION={version}");
}

fn git_output(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}
