//! Literal script generation and OpenCode's global inventory contract.
use crate::config::Host;
use crate::remote::{CheckedReader, Deadline};
use crate::secure::Dir;
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::HashSet;
use std::io::BufReader;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn script(host: &Host, harness: &str) -> Result<String> {
    let (roots, automatic, executable) = match harness {
        "claude" => (
            &host.claude_paths,
            "${CLAUDE_CONFIG_DIR:-\"$HOME/.claude\"}",
            "claude",
        ),
        "codex" => (
            &host.codex_paths,
            "${CODEX_HOME:-\"$HOME/.codex\"}",
            "codex",
        ),
        "pi" => (
            &host.pi_paths,
            "${PI_CODING_AGENT_SESSION_DIR:-\"${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/sessions\"}",
            "pi",
        ),
        "opencode" => return Ok(opencode_inventory()),
        _ => bail!("unsupported harness"),
    };
    let arguments = if roots.is_empty() {
        format!("\"{automatic}\"")
    } else {
        roots.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    };
    Ok(format!(
        "harness={}\nexecutable={}\nrequired={}\nset -- {}\n{}",
        quote(harness),
        quote(executable),
        u8::from(!roots.is_empty()),
        arguments,
        include_str!("files.sh")
    ))
}

const OPEN_PRELUDE: &str = r#"set -efu
umask 077
printf 'GLURP1\000'
if ! command -v opencode >/dev/null 2>&1; then printf 'T\000not-found\000E\000'; exit 0; fi
tmp=$(mktemp -d) || exit 2
trap 'rm -rf "$tmp"' 0
trap 'exit 2' 1 2 15
emit() {
    size=$(wc -c < "$tmp/output") || exit 2
    name=$1
    set -- $size
    printf 'F\000%s\000%s\000' "$name" "$1"
    cat "$tmp/output" || exit 2
}
"#;

fn opencode_inventory() -> String {
    let check = r#"database=$(opencode db path) || exit 2
case "$database" in /*) ;; *) exit 2;; esac
check=$database
while :; do
    [ ! -L "$check" ] || exit 2
    [ "$check" != / ] || break
    check=${check%/*}; [ -n "$check" ] || check=/
done
[ -f "$database" ] && [ -r "$database" ] || exit 2
opencode db 'SELECT id FROM session ORDER BY id' --format json > "$tmp/output" || exit 2
emit inventory.json
printf 'T\000ok\000E\000'
"#;
    format!("{OPEN_PRELUDE}{check}")
}

pub fn inventory(directory: &Dir, deadline: &Deadline) -> Result<Vec<String>> {
    let value: Value = serde_json::from_reader(BufReader::new(CheckedReader {
        reader: directory.read("inventory.json")?,
        deadline,
    }))
    .map_err(|_| anyhow::anyhow!("OpenCode inventory is not valid JSON"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("OpenCode inventory must be an array"))?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for row in rows {
        deadline.check()?;
        let obj = row
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("OpenCode inventory row must be an object"))?;
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("OpenCode inventory id missing"))?;
        if obj.len() != 1
            || !id.starts_with("ses_")
            || id.len() <= 4
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !seen.insert(id.to_owned())
        {
            bail!("OpenCode inventory has invalid or duplicate IDs");
        }
        ids.push(id.to_owned());
    }
    ids.sort();
    Ok(ids)
}

pub fn exports(ids: &[String]) -> String {
    let mut script = OPEN_PRELUDE.to_owned();
    // Once an inventory succeeds, a disappeared executable is a failure.
    script = script.replace("printf 'T\\000not-found\\000E\\000'; exit 0", "exit 2");
    for id in ids {
        script.push_str(&format!(
            "opencode export {} > \"$tmp/output\" || exit 2\nemit {}\n",
            quote(id),
            quote(&format!("{id}.json"))
        ));
    }
    script.push_str("printf 'T\\000ok\\000E\\000'\n");
    script
}

pub fn validate_exports(
    directory: &Dir,
    ids: &[String],
    paths: &[String],
    deadline: &Deadline,
) -> Result<()> {
    if paths.len() != ids.len() {
        bail!("OpenCode exports do not match inventory");
    }
    let paths: HashSet<_> = paths.iter().collect();
    for id in ids {
        deadline.check()?;
        let path = format!("{id}.json");
        if !paths.contains(&path) {
            bail!("OpenCode export missing");
        }
        let value: Value = serde_json::from_reader(BufReader::new(CheckedReader {
            reader: directory.read(&path)?,
            deadline,
        }))
        .map_err(|_| anyhow::anyhow!("OpenCode export is not one valid JSON document"))?;
        if value
            .get("info")
            .and_then(|i| i.get("id"))
            .and_then(Value::as_str)
            != Some(id)
            || !value.get("messages").is_some_and(Value::is_array)
        {
            bail!("OpenCode export schema or session identity mismatch");
        }
        for message in value["messages"].as_array().unwrap() {
            deadline.check()?;
            if !message.get("info").is_some_and(Value::is_object)
                || !message.get("parts").is_some_and(Value::is_array)
            {
                bail!("OpenCode export message schema mismatch");
            }
        }
    }
    Ok(())
}
