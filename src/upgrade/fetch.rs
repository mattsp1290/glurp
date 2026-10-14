//! Release downloads through a `curl` subprocess, so no TLS stack is linked.
use anyhow::{Error, Result};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

const MANIFEST_LIMIT: u64 = 64 * 1024;

fn curl(base: &str, max_time: &str) -> Command {
    let mut curl = Command::new("curl");
    // `-q` must come first: a user's ~/.curlrc could otherwise change the
    // output. `--globoff` keeps `{}` and `[]` in a URL literal. Bound both
    // individual attempts and the retry window.
    curl.args(["-q", "-fsSL", "--globoff", "--connect-timeout", "10"])
        .args(["--retry", "3", "--retry-max-time", max_time])
        .args(["--max-time", max_time])
        .stdin(Stdio::null());
    if base.starts_with("https://") {
        curl.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    curl
}

fn curl_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::msg("glurp upgrade requires curl")
    } else {
        Error::msg(format!("curl: {error}"))
    }
}

fn curl_failure(what: &str, url: &str, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr);
    Error::msg(format!("could not fetch {what} {url}: {}", detail.trim()))
}

pub(super) fn fetch_manifest(base: &str, url: &str) -> Result<Vec<u8>> {
    let mut child = curl(base, "20")
        .args(["--url", url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(curl_error)?;
    let mut body = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut body);
    if body.len() as u64 > MANIFEST_LIMIT {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::msg("release manifest too large"));
    }
    let output = child.wait_with_output().map_err(curl_error)?;
    read.map_err(curl_error)?;
    if !output.status.success() {
        return Err(curl_failure("release manifest", url, &output.stderr));
    }
    Ok(body)
}

pub(super) fn download(base: &str, url: &str, destination: &Path) -> Result<()> {
    let output = curl(base, "300")
        .arg("-o")
        .arg(destination)
        .args(["--url", url])
        .output()
        .map_err(curl_error)?;
    if !output.status.success() {
        return Err(curl_failure("release binary", url, &output.stderr));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_ignores_user_configuration_and_bounds_fetches() {
        let command = curl("https://example.org/releases", "20");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args[0], "-q");
        assert!(args.contains(&"-fsSL"));
        assert!(args.contains(&"--globoff"));
        for pair in [
            ["--connect-timeout", "10"],
            ["--retry", "3"],
            ["--retry-max-time", "20"],
            ["--max-time", "20"],
            ["--proto", "=https"],
            ["--proto-redir", "=https"],
        ] {
            assert!(args.windows(2).any(|args| args == pair));
        }
    }
}
