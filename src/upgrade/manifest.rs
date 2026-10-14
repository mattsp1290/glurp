//! The schema 1 release manifest: which base URLs, targets and assets are
//! acceptable. `docs/release.md` holds the contract.
use super::{
    BASE_URL_ENV,
    version::{Version, parse_version},
};
use anyhow::{Error, Result};
use std::collections::BTreeMap;

pub(super) fn target() -> Result<String> {
    use std::env::consts::{ARCH, OS};
    if matches!(OS, "linux" | "macos") && matches!(ARCH, "x86_64" | "aarch64") {
        Ok(format!("{OS}-{ARCH}"))
    } else {
        Err(Error::msg(format!("no release binary for {OS}-{ARCH}")))
    }
}

pub(super) fn release_base(raw: &str) -> Result<&str> {
    let base = raw.trim_end_matches('/');
    if base.starts_with("https://") || base.starts_with("file://") {
        Ok(base)
    } else {
        Err(Error::msg(format!(
            "{BASE_URL_ENV} must start with https:// or file://"
        )))
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    version: String,
    tag: String,
    assets: BTreeMap<String, String>,
    sha256: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(super) struct Release {
    pub(super) tag: String,
    pub(super) version: Version,
    pub(super) url: String,
    pub(super) sha256: String,
}

/// Checks a schema 1 manifest and selects `target`. `requested` is the tag
/// given with `--version`.
pub(super) fn validate(
    raw: &[u8],
    base: &str,
    target: &str,
    requested: Option<&str>,
) -> Result<Release> {
    let invalid = |reason: &str| Error::msg(format!("invalid release manifest: {reason}"));
    let json: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| invalid(&e.to_string()))?;
    if json["schema"] != 1 {
        return Err(Error::msg("unsupported release manifest schema"));
    }
    let manifest: Manifest = serde_json::from_value(json).map_err(|e| invalid(&e.to_string()))?;
    let version = parse_version(&manifest.version).ok_or_else(|| invalid("version"))?;
    let tag = manifest.tag;
    if tag != format!("v{}", manifest.version) {
        return Err(invalid("tag does not match version"));
    }
    if requested.is_some_and(|requested| requested != tag) {
        return Err(invalid(&format!("it describes {tag}")));
    }
    if !manifest.assets.keys().eq(manifest.sha256.keys()) {
        return Err(invalid("assets and sha256 name different targets"));
    }
    let (Some(url), Some(sha256)) = (manifest.assets.get(target), manifest.sha256.get(target))
    else {
        return Err(Error::msg(format!(
            "no release binary for {target} in {tag}"
        )));
    };
    if *url != format!("{base}/download/{tag}/glurp-{target}") {
        return Err(invalid("asset URL is outside the release"));
    }
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    if sha256.len() != 64 || !sha256.bytes().all(hex) {
        return Err(invalid("sha256"));
    }
    Ok(Release {
        tag,
        version,
        url: url.clone(),
        sha256: sha256.clone(),
    })
}

/// A valid manifest for one target, for tests in this module tree.
#[cfg(test)]
pub(super) fn sample(base: &str, tag: &str, target: &str, digest: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "version": tag.trim_start_matches('v'),
        "tag": tag,
        "assets": {target: format!("{base}/download/{tag}/glurp-{target}")},
        "sha256": {target: digest},
    })
}

#[cfg(test)]
mod tests {
    use super::{sample as manifest, *};

    const BASE: &str = "https://example.org/releases";
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn checked(manifest: &serde_json::Value, requested: Option<&str>) -> Result<Release> {
        validate(
            manifest.to_string().as_bytes(),
            BASE,
            "linux-x86_64",
            requested,
        )
    }

    #[test]
    fn manifest_validation_fails_closed() {
        let good = manifest(BASE, "v1.2.3", "linux-x86_64", DIGEST);
        let release = checked(&good, Some("v1.2.3")).unwrap();
        assert_eq!(release.version, (1, 2, 3));
        assert_eq!(
            release.url,
            format!("{BASE}/download/v1.2.3/glurp-linux-x86_64")
        );
        let broken = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut manifest = good.clone();
            edit(&mut manifest);
            checked(&manifest, None).unwrap_err().to_string()
        };
        assert!(broken(&|m| m["schema"] = 2.into()).contains("schema"));
        assert!(broken(&|m| m["schema"] = "1".into()).contains("schema"));
        assert!(broken(&|m| m["version"] = "1.2".into()).contains("version"));
        assert!(broken(&|m| m["tag"] = "v1.2.4".into()).contains("tag"));
        assert!(
            broken(&|m| m["sha256"]["macos-aarch64"] = DIGEST.into()).contains("different targets")
        );
        assert!(broken(&|m| m["sha256"]["linux-x86_64"] = "abc".into()).contains("sha256"));
        assert!(
            broken(&|m| m["sha256"]["linux-x86_64"] = DIGEST.to_uppercase().into())
                .contains("sha256")
        );
        for url in [
            "https://example.com/releases/download/v1.2.3/glurp-linux-x86_64".to_owned(),
            format!("{BASE}/download/../x/glurp-linux-x86_64"),
            format!("{BASE}/download/v1.2.3/glurp-linux-x86_64?x"),
        ] {
            assert!(
                broken(&|m| m["assets"]["linux-x86_64"] = url.clone().into()).contains("outside"),
                "{url}"
            );
        }
        assert!(
            checked(&good, Some("v1.2.4"))
                .unwrap_err()
                .to_string()
                .contains("v1.2.3")
        );
        let missing = validate(good.to_string().as_bytes(), BASE, "macos-aarch64", None);
        assert!(missing.unwrap_err().to_string().contains("macos-aarch64"));
        assert!(validate(b"not json", BASE, "linux-x86_64", None).is_err());
    }

    #[test]
    fn base_url_needs_https_or_file_and_loses_trailing_slashes() {
        assert_eq!(release_base("https://example.org/r//").unwrap(), {
            "https://example.org/r"
        });
        assert_eq!(release_base("file:///tmp/r/").unwrap(), "file:///tmp/r");
        for bad in ["http://example.org/r", "example.org", "", "file://"] {
            assert!(release_base(bad).is_err(), "{bad}");
        }
    }
}
