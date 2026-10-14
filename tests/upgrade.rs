//! `glurp upgrade` control flow against `file://` release trees. The released
//! "binary" is a shell script; real binaries and HTTPS are covered by the
//! release workflow's smoke job.
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

const TEMP_PREFIX: &str = ".glurp-upgrade-";

fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
fn script(version: &str) -> String {
    format!("#!/bin/sh\necho 'glurp {version}'\n")
}
fn sha256(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

/// One sandbox holding a release tree and a private copy of the built `glurp`.
/// `upgrade` never runs on the Cargo-built binary itself.
struct Releases {
    /// Removes the tree on drop.
    _sandbox: tempfile::TempDir,
    /// Canonical, because `glurp` reports its own canonical path.
    root: PathBuf,
    glurp: PathBuf,
    original: Vec<u8>,
}
impl Releases {
    /// `None` when `curl` is absent. Publishes `latest` as the latest release.
    fn new(latest: &str) -> Option<Self> {
        Self::installed_at("install/glurp", latest)
    }
    fn installed_at(relative: &str, latest: &str) -> Option<Self> {
        if Command::new("curl").arg("--version").output().is_err() {
            assert!(std::env::var_os("CI").is_none(), "CI must provide curl");
            eprintln!("skipping: curl is not on PATH");
            return None;
        }
        let sandbox = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let glurp = root.join(relative);
        fs::create_dir_all(glurp.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_glurp"), &glurp).unwrap();
        let releases = Self {
            original: fs::read(&glurp).unwrap(),
            _sandbox: sandbox,
            root,
            glurp,
        };
        let manifest = releases.publish(latest, &script(latest));
        let dir = releases.dir("latest/download");
        fs::create_dir_all(&dir).unwrap();
        fs::copy(manifest, dir.join("glurp-manifest.json")).unwrap();
        Some(releases)
    }
    fn dir(&self, relative: &str) -> PathBuf {
        self.root.join("releases").join(relative)
    }
    fn base(&self) -> String {
        format!("file://{}/releases", self.root.display())
    }
    fn asset(&self, tag: &str) -> PathBuf {
        self.dir(&format!("download/{tag}/glurp-{}", target()))
    }
    /// Writes the pinned tree for `tag` and returns its manifest path.
    fn publish(&self, tag: &str, binary: &str) -> PathBuf {
        let asset = self.asset(tag);
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::write(&asset, binary).unwrap();
        let manifest = serde_json::json!({
            "schema": 1,
            "version": tag.trim_start_matches('v'),
            "tag": tag,
            "assets": {target(): format!("{}/download/{tag}/glurp-{}", self.base(), target())},
            "sha256": {target(): sha256(&asset)},
        });
        let path = asset.with_file_name("glurp-manifest.json");
        fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        path
    }
    fn latest_manifest(&self) -> PathBuf {
        self.dir("latest/download/glurp-manifest.json")
    }
    fn command(&self, base: &str, args: &[&str]) -> Command {
        let mut command = Command::new(&self.glurp);
        command
            .args(args)
            .env("GLURP_RELEASE_BASE_URL", base)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env_remove("CARGO_HOME")
            .current_dir(&self.root);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        output(&mut self.command(&self.base(), args))
    }
    fn unchanged(&self) -> bool {
        fs::read(&self.glurp).unwrap() == self.original && self.temps().is_empty()
    }
    fn no_state_created(&self) -> bool {
        ["home", "config", "data"]
            .iter()
            .all(|name| !self.root.join(name).exists())
    }
    fn temps(&self) -> Vec<String> {
        fs::read_dir(self.glurp.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(TEMP_PREFIX))
            .collect()
    }
}
/// A sibling test forking between this copy's write and close can hold the
/// descriptor briefly, which makes the first exec fail with ETXTBSY.
fn output(command: &mut Command) -> Output {
    for _ in 0..50 {
        match command.output() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result.unwrap(),
        }
    }
    panic!("copied glurp stayed busy");
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn failed(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    stderr(output)
}

#[test]
fn upgrade_force_replaces_the_binary_in_place() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let out = r.run(&["upgrade", "--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("installed glurp v9.9.9 at"), "{stdout}");
    assert_eq!(fs::read_to_string(&r.glurp).unwrap(), script("v9.9.9"));
    let mode = fs::metadata(&r.glurp).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(r.temps().is_empty());
    assert!(r.no_state_created());
}

#[test]
fn upgrade_version_installs_the_pinned_release() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    r.publish("v0.0.0", &script("v0.0.0"));
    let out = r.run(&["upgrade", "--version", "v0.0.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // The root `--version` banner would print `glurp <GLURP_VERSION>` and install nothing.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.starts_with("glurp "), "{stdout}");
    assert_eq!(fs::read_to_string(&r.glurp).unwrap(), script("v0.0.0"));
    assert!(r.no_state_created());
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    assert!(failed(&r.run(&["upgrade", "--version", "9.9.9"])).contains("vX.Y.Z"));
    // A pinned manifest that describes another release is refused.
    fs::create_dir_all(r.dir("download/v9.9.7")).unwrap();
    fs::copy(
        r.latest_manifest(),
        r.dir("download/v9.9.7/glurp-manifest.json"),
    )
    .unwrap();
    assert!(failed(&r.run(&["upgrade", "--version", "v9.9.7"])).contains("v9.9.9"));
    assert!(r.unchanged());
}

#[test]
fn upgrade_check_reports_json_without_configuration_or_home() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let out = output(
        r.command(&r.base(), &["upgrade", "--check", "--json"])
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME"),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "current",
            "latest",
            "path",
            "target",
            "update_available",
            "upgraded"
        ]
    );
    assert_eq!(report["latest"], "v9.9.9");
    assert_eq!(report["target"], target());
    assert_eq!(report["path"], r.glurp.to_str().unwrap());
    assert_eq!(report["upgraded"], false);
    assert!(report["update_available"].is_boolean());
    assert!(r.unchanged());
    assert!(r.no_state_created());
    // A trailing slash on the base names the same release tree.
    let slashed =
        output(&mut r.command(&format!("{}/", r.base()), &["upgrade", "--check", "--json"]));
    assert!(slashed.status.success(), "{}", stderr(&slashed));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&slashed.stdout).unwrap(),
        report
    );
}

#[test]
fn upgrade_failures_leave_the_binary_and_directory_untouched() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    fs::write(r.asset("v9.9.9"), "tampered\n").unwrap();
    assert!(failed(&r.run(&["upgrade", "--force"])).contains("checksum"));
    assert!(r.unchanged());

    // The checksum matches but the binary reports another version.
    let manifest = r.publish("v9.9.9", &script("v1.1.1"));
    fs::copy(manifest, r.latest_manifest()).unwrap();
    assert!(failed(&r.run(&["upgrade", "--force"])).contains("glurp v1.1.1"));
    assert!(r.unchanged());

    let text = fs::read_to_string(r.latest_manifest()).unwrap();
    fs::write(r.latest_manifest(), text.replace(&target(), "plan9-mips")).unwrap();
    assert!(failed(&r.run(&["upgrade", "--force"])).contains(&target()));
    assert!(r.unchanged());

    fs::write(r.latest_manifest(), " ".repeat(64 * 1024 + 1)).unwrap();
    assert!(failed(&r.run(&["upgrade", "--force"])).contains("too large"));
    assert!(r.unchanged());

    let unreachable = output(&mut r.command("file:///nonexistent", &["upgrade", "--force"]));
    failed(&unreachable);
    let insecure = output(&mut r.command("http://example.invalid/r", &["upgrade", "--force"]));
    assert!(failed(&insecure).contains("https://"));
    assert!(r.unchanged());
}

#[test]
fn malformed_manifests_are_rejected_without_changing_the_install() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(r.latest_manifest()).unwrap()).unwrap();
    let mut cases = Vec::new();
    for (field, value) in [
        ("schema", serde_json::json!(2)),
        ("schema", serde_json::json!("1")),
        ("version", serde_json::json!("09.9.9")),
        ("version", serde_json::json!("9.9.9-rc1")),
        ("tag", serde_json::json!("v9.9.8")),
    ] {
        let mut manifest = valid.clone();
        manifest[field] = value;
        cases.push(manifest.to_string());
    }
    for digest in ["bad".to_owned(), "A".repeat(64)] {
        let mut manifest = valid.clone();
        manifest["sha256"][target()] = digest.into();
        cases.push(manifest.to_string());
    }
    let mut manifest = valid.clone();
    manifest["assets"][target()] = "https://example.invalid/glurp".into();
    cases.push(manifest.to_string());
    let mut manifest = valid;
    manifest["sha256"]["extra-target"] = "0".repeat(64).into();
    cases.push(manifest.to_string());
    cases.push("not json".into());
    for malformed in cases {
        fs::write(r.latest_manifest(), &malformed).unwrap();
        assert!(
            failed(&r.run(&["upgrade", "--force"])).contains("manifest"),
            "{malformed}"
        );
        assert!(r.unchanged(), "{malformed}");
        assert!(r.no_state_created());
    }
}

#[test]
fn missing_curl_leaves_the_binary_and_directory_untouched() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let out = output(
        r.command(&r.base(), &["upgrade", "--force"])
            .env("PATH", r.root.join("no-tools")),
    );
    assert!(failed(&out).contains("requires curl"));
    assert!(r.unchanged());
    assert!(r.no_state_created());
}

#[test]
fn an_interrupted_binary_download_removes_partial_files() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let bin = r.root.join("tools");
    fs::create_dir(&bin).unwrap();
    let curl = bin.join("curl");
    fs::write(
        &curl,
        r#"#!/bin/sh
destination=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; destination=$1;;
        --url) shift; url=$1;;
    esac
    shift
done
if [ -n "$destination" ]; then
    printf 'partial binary' > "$destination"
    printf 'simulated interrupted transfer' >&2
    exit 28
fi
exec /bin/cat "${url#file://}"
"#,
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    let out = output(
        r.command(&r.base(), &["upgrade", "--force"])
            .env("PATH", bin),
    );
    assert!(failed(&out).contains("could not fetch release binary"));
    assert!(r.unchanged());
}

#[test]
fn check_with_force_and_a_pinned_version_never_installs() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    r.publish("v0.0.0", &script("v0.0.0"));
    let out = r.run(&[
        "upgrade",
        "--check",
        "--force",
        "--version",
        "v0.0.0",
        "--json",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["latest"], "v0.0.0");
    assert_eq!(report["upgraded"], false);
    assert!(r.unchanged());
}

#[test]
fn upgrade_json_emits_one_report_after_successful_replacement() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    let out = r.run(&["upgrade", "--force", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["latest"], "v9.9.9");
    assert_eq!(report["upgraded"], true);
    assert_eq!(fs::read_to_string(&r.glurp).unwrap(), script("v9.9.9"));
    assert!(r.temps().is_empty());
}

#[test]
fn upgrade_reports_a_read_only_install_directory() {
    let Some(r) = Releases::new("v9.9.9") else {
        return;
    };
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: root ignores directory permissions");
        return;
    }
    let dir = r.glurp.parent().unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let out = r.run(&["upgrade", "--force"]);
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(failed(&out).contains("install directory not writable"));
    assert!(r.unchanged());
}

#[test]
fn upgrade_refuses_build_tree_binaries_unless_forced() {
    let Some(r) = Releases::installed_at("target/release/glurp", "v9.9.9") else {
        return;
    };
    assert!(failed(&r.run(&["upgrade"])).contains("--force"));
    assert!(r.unchanged());
    // Reporting is always allowed.
    assert!(r.run(&["upgrade", "--check"]).status.success());
    let out = r.run(&["upgrade", "--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(&r.glurp).unwrap(), script("v9.9.9"));
}

#[test]
fn upgrade_refuses_cargo_installations_unless_forced() {
    let Some(r) = Releases::installed_at("cargo/bin/glurp", "v9.9.9") else {
        return;
    };
    let command = |args: &[&str]| {
        let mut command = r.command(&r.base(), args);
        command.env("CARGO_HOME", r.root.join("cargo"));
        command
    };
    let out = output(&mut command(&["upgrade"]));
    assert!(failed(&out).contains("installed by Cargo"));
    assert!(r.unchanged());
    let out = output(&mut command(&["upgrade", "--check"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(r.unchanged());
    let out = output(&mut command(&["upgrade", "--force"]));
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(&r.glurp).unwrap(), script("v9.9.9"));
}

#[test]
fn upgrade_help_describes_check_force_json_and_pinning() {
    let out = Command::new(env!("CARGO_BIN_EXE_glurp"))
        .args(["upgrade", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--check", "--force", "--json", "--version <TAG>"] {
        assert!(help.contains(flag), "missing {flag}: {help}");
    }
}
