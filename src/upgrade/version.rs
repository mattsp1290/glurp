//! Release version grammar and the upgrade decision.

pub(super) type Version = (u64, u64, u64);

pub(super) fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.split('.').map(|part| {
        // No sign, no empty component and no leading zero: one spelling per version.
        let decimal = !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'));
        decimal.then(|| part.parse::<u64>().ok()).flatten()
    });
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub(super) fn parse_tag(tag: &str) -> Option<Version> {
    parse_version(tag.strip_prefix('v')?)
}

/// The release a `git describe` version was built from: `vA.B.C`, optionally
/// followed by `-N-g<sha>` and then `-dirty`.
fn build_base(current: &str) -> Option<Version> {
    let described = current.strip_suffix("-dirty").unwrap_or(current);
    let mut parts = described.splitn(3, '-');
    let base = parse_tag(parts.next()?)?;
    match (parts.next(), parts.next()) {
        (None, _) => Some(base),
        (Some(count), Some(sha)) => {
            let hex = sha.strip_prefix('g')?;
            let described = !count.is_empty()
                && count.bytes().all(|b| b.is_ascii_digit())
                && !hex.is_empty()
                && hex.bytes().all(|b| b.is_ascii_hexdigit());
            described.then_some(base)
        }
        (Some(_), None) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Decision {
    Upgrade,
    UpToDate,
    /// The running build has no release version to compare with.
    Unknown,
}

pub(super) fn decide(current: &str, latest: Version) -> Decision {
    match build_base(current) {
        Some(base) if latest > base => Decision::Upgrade,
        Some(_) => Decision::UpToDate,
        None => Decision::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_and_reject_other_shapes() {
        assert_eq!(parse_tag("v0.10.2"), Some((0, 10, 2)));
        assert!(parse_tag("v0.10.2") > parse_tag("v0.9.9"));
        for bad in [
            "0.1.2",
            "v1.2",
            "v1.2.3.4",
            "v1.2.x",
            "v1..3",
            "v1.2.3-rc1",
            "v+1.2.3",
            "v01.2.3",
            "v1.2.03",
            "",
        ] {
            assert_eq!(parse_tag(bad), None, "{bad}");
        }
    }

    #[test]
    fn decision_table_follows_the_running_build() {
        use Decision::*;
        let latest = (1, 2, 3);
        for (current, expected) in [
            ("v1.2.2", Upgrade),
            ("v1.2.3", UpToDate),
            ("v1.10.0", UpToDate),
            ("v1.2.2-dirty", Upgrade),
            ("v1.2.3-dirty", UpToDate),
            ("v1.2.2-4-gabc1234", Upgrade),
            ("v1.2.2-4-gabc1234-dirty", Upgrade),
            ("v1.2.3-4-gabc1234", UpToDate),
            ("v1.3.0-1-gabc1234-dirty", UpToDate),
            ("dev", Unknown),
            ("abc1234", Unknown),
            ("abc1234-dirty", Unknown),
            ("v1.2.2-rc1", Unknown),
            ("v1.2.2-4-abc1234", Unknown),
            ("v1.2.2-x-gabc1234", Unknown),
        ] {
            assert_eq!(decide(current, latest), expected, "{current}");
        }
    }
}
