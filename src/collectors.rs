//! Literal script generation and OpenCode's global inventory contract.
use crate::collection::Harness;
use crate::config::Host;
use crate::error::{CoreError, TaskErrorKind, with_kind};
use crate::remote::{CheckedReader, Deadline};
use crate::secure::Dir;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufReader, Read};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn script(host: &Host, harness: Harness) -> String {
    let (roots, automatic, executable) = match harness {
        Harness::Claude => (
            &host.claude_paths,
            "${CLAUDE_CONFIG_DIR:-\"$HOME/.claude\"}",
            "claude",
        ),
        Harness::Codex => (
            &host.codex_paths,
            "${CODEX_HOME:-\"$HOME/.codex\"}",
            "codex",
        ),
        Harness::Pi => (
            &host.pi_paths,
            "${PI_CODING_AGENT_SESSION_DIR:-\"${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/sessions\"}",
            "pi",
        ),
        Harness::OpenCode => return opencode_inventory(),
    };
    let arguments = if roots.is_empty() {
        format!("\"{automatic}\"")
    } else {
        roots.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    };
    format!(
        "harness={}\nexecutable={}\nrequired={}\nset -- {}\n{}",
        quote(harness.as_str()),
        quote(executable),
        u8::from(!roots.is_empty()),
        arguments,
        include_str!("files.sh")
    )
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

// A JSON decoder may erase io::Error's cause. Keep the checked reader alive
// until the decode result has been classified, and use only a failure it actually
// observed. Token state at this point is not evidence of the parser's cause.
fn decoded_json(decoded: serde_json::Result<Value>, failure: Option<CoreError>) -> Result<Value> {
    decoded.map_err(|_| {
        failure
            .unwrap_or(CoreError::Task(TaskErrorKind::InvalidRemoteStream))
            .into()
    })
}

fn read_json(reader: impl Read, deadline: &Deadline) -> Result<Value> {
    let mut checked = CheckedReader::new(reader, deadline);
    let decoded = serde_json::from_reader(BufReader::new(&mut checked));
    decoded_json(decoded, checked.failure())
}

pub fn inventory(directory: &Dir, deadline: &Deadline) -> Result<Vec<String>> {
    inventory_from_reader(
        directory
            .read("inventory.json")
            .map_err(|error| with_kind(error, TaskErrorKind::LocalStorage))?,
        deadline,
    )
}

fn inventory_from_reader(reader: impl Read, deadline: &Deadline) -> Result<Vec<String>> {
    let value = read_json(reader, deadline)?;
    let rows = value
        .as_array()
        .ok_or(CoreError::Task(TaskErrorKind::InvalidRemoteStream))?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for row in rows {
        deadline.check()?;
        let obj = row
            .as_object()
            .ok_or(CoreError::Task(TaskErrorKind::InvalidRemoteStream))?;
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .ok_or(CoreError::Task(TaskErrorKind::InvalidRemoteStream))?;
        if obj.len() != 1
            || !id.starts_with("ses_")
            || id.len() <= 4
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !seen.insert(id.to_owned())
        {
            return Err(CoreError::Task(TaskErrorKind::InvalidRemoteStream).into());
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
        return Err(CoreError::Task(TaskErrorKind::InvalidRemoteStream).into());
    }
    let paths: HashSet<_> = paths.iter().collect();
    for id in ids {
        deadline.check()?;
        let path = format!("{id}.json");
        if !paths.contains(&path) {
            return Err(CoreError::Task(TaskErrorKind::InvalidRemoteStream).into());
        }
        validate_export_from_reader(
            directory
                .read(&path)
                .map_err(|error| with_kind(error, TaskErrorKind::LocalStorage))?,
            id,
            deadline,
        )?;
    }
    Ok(())
}

fn validate_export_from_reader(reader: impl Read, id: &str, deadline: &Deadline) -> Result<()> {
    let value = read_json(reader, deadline)?;
    if value
        .get("info")
        .and_then(|i| i.get("id"))
        .and_then(Value::as_str)
        != Some(id)
        || !value.get("messages").is_some_and(Value::is_array)
    {
        return Err(CoreError::Task(TaskErrorKind::InvalidRemoteStream).into());
    }
    // The schema check above establishes this value is an array.
    let messages = value["messages"]
        .as_array()
        .ok_or(CoreError::Task(TaskErrorKind::InvalidRemoteStream))?;
    for message in messages {
        deadline.check()?;
        if !message.get("info").is_some_and(Value::is_object)
            || !message.get("parts").is_some_and(Value::is_array)
        {
            return Err(CoreError::Task(TaskErrorKind::InvalidRemoteStream).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classify;
    use std::cell::Cell;
    use std::io::{Cursor, Write};
    use tokio_util::sync::CancellationToken;

    const PRIVATE: &str = "SYNTHETIC_PRIVATE_TRANSCRIPT";

    fn deadline() -> Deadline {
        Deadline::new(60, CancellationToken::new())
    }

    fn assert_category(error: anyhow::Error, expected: CoreError) {
        let cause = classify(&error);
        assert_eq!(cause, expected);
        assert!(!format!("{cause} {cause:?}").contains(PRIVATE));
        assert!(std::error::Error::source(&cause).is_none());
        if let CoreError::Task(kind) = cause {
            assert!(!serde_json::to_string(&kind).unwrap().contains(PRIVATE));
            assert!(std::error::Error::source(&kind).is_none());
        }
        // The decoder must discard serde_json's potentially sensitive excerpts,
        // including in the private chain, rather than only masking Display.
        assert!(!format!("{error:#?}").contains(PRIVATE));
    }

    /// Advance only this call's state from inside a successful decoder read.
    /// The next attempted read is interrupted by CheckedReader, after JSON
    /// decoding has begun. No background timer or global hook orders the test.
    struct InterruptRead<'a> {
        prefix: &'a [u8],
        calls: &'a Cell<usize>,
        deadline: &'a Deadline,
        cancellation: Option<CancellationToken>,
    }
    impl Read for InterruptRead<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            assert_eq!(self.calls.get(), 0, "reader continued after interruption");
            assert!(self.prefix.len() <= bytes.len());
            self.calls.set(self.calls.get() + 1);
            bytes[..self.prefix.len()].copy_from_slice(self.prefix);
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
            } else {
                self.deadline.expire_for_test();
            }
            Ok(self.prefix.len())
        }
    }

    fn interrupt_decoder(export: bool, cancel: bool) {
        let token = CancellationToken::new();
        let deadline = Deadline::new(60, token.clone());
        let calls = Cell::new(0);
        let prefix = if export {
            format!(r#"{{"info":{{"id":"ses_test"}},"messages":[{{"info":{{"text":"{PRIVATE}"#)
        } else {
            format!(r#"[{{"id":"ses_{PRIVATE}"#)
        };
        let reader = InterruptRead {
            prefix: prefix.as_bytes(),
            calls: &calls,
            deadline: &deadline,
            cancellation: cancel.then_some(token),
        };
        let error = if export {
            validate_export_from_reader(reader, "ses_test", &deadline).unwrap_err()
        } else {
            inventory_from_reader(reader, &deadline).unwrap_err()
        };
        assert_eq!(
            calls.get(),
            1,
            "interruption happened before decoding began"
        );
        assert_category(
            error,
            if cancel {
                CoreError::Cancelled
            } else {
                CoreError::Task(TaskErrorKind::Timeout)
            },
        );
    }

    #[test]
    fn inventory_decoder_read_retains_cancellation() {
        interrupt_decoder(false, true);
    }

    #[test]
    fn inventory_decoder_read_retains_timeout() {
        interrupt_decoder(false, false);
    }

    #[test]
    fn export_decoder_read_retains_cancellation() {
        interrupt_decoder(true, true);
    }

    #[test]
    fn export_decoder_read_retains_timeout() {
        interrupt_decoder(true, false);
    }

    #[test]
    fn late_cancellation_does_not_reclassify_json_syntax_failure() {
        let token = CancellationToken::new();
        let deadline = Deadline::new(60, token.clone());
        let malformed = format!("!{PRIVATE}");
        let mut checked = CheckedReader::new(Cursor::new(malformed), &deadline);
        let decoded = serde_json::from_reader(BufReader::new(&mut checked));
        assert!(decoded.is_err());
        assert_eq!(checked.failure(), None);
        token.cancel();
        assert_category(
            decoded_json(decoded, checked.failure()).unwrap_err(),
            CoreError::Task(TaskErrorKind::InvalidRemoteStream),
        );
    }

    #[test]
    fn late_timeout_does_not_reclassify_json_syntax_failure() {
        let deadline = deadline();
        let malformed = format!("!{PRIVATE}");
        let mut checked = CheckedReader::new(Cursor::new(malformed), &deadline);
        let decoded = serde_json::from_reader(BufReader::new(&mut checked));
        assert!(decoded.is_err());
        deadline.expire_for_test();
        assert_category(
            decoded_json(decoded, checked.failure()).unwrap_err(),
            CoreError::Task(TaskErrorKind::InvalidRemoteStream),
        );
    }

    #[test]
    fn inventory_accepts_sorted_unique_session_ids() {
        let ids = inventory_from_reader(
            Cursor::new(br#"[{"id":"ses_z"},{"id":"ses_1"},{"id":"ses_A"}]"#),
            &deadline(),
        )
        .unwrap();
        assert_eq!(ids, ["ses_1", "ses_A", "ses_z"]);
        assert!(
            inventory_from_reader(Cursor::new(b"[]"), &deadline())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_inventory_is_redacted_invalid_remote_stream() {
        let too_long = format!("ses_{}", "a".repeat(125));
        for input in [
            format!("!{PRIVATE}"),
            format!(r#"[{{"id":"ses_{PRIVATE}"}}] trailing"#),
            format!(r#"{{"{PRIVATE}":[]}}"#),
            format!(r#"["{PRIVATE}"]"#),
            format!(r#"[{{"{PRIVATE}":"ses_missing"}}]"#),
            format!(r#"[{{"id":8,"{PRIVATE}":true}}]"#),
            format!(r#"[{{"id":"ses_ok","{PRIVATE}":true}}]"#),
            r#"[{"id":"ses_"}]"#.to_owned(),
            format!(r#"[{{"id":"{PRIVATE}"}}]"#),
            format!(r#"[{{"id":"ses_{PRIVATE}/path"}}]"#),
            format!(r#"[{{"id":"ses_{PRIVATE}"}},{{"id":"ses_{PRIVATE}"}}]"#),
            format!(r#"[{{"id":"{too_long}"}}]"#),
        ] {
            assert_category(
                inventory_from_reader(Cursor::new(input), &deadline()).unwrap_err(),
                CoreError::Task(TaskErrorKind::InvalidRemoteStream),
            );
        }
    }

    #[test]
    fn export_accepts_the_matching_session_and_message_schema() {
        for input in [
            r#"{"info":{"id":"ses_test"},"messages":[]}"#,
            r#"{"info":{"id":"ses_test"},"messages":[{"info":{},"parts":[] }]}"#,
        ] {
            validate_export_from_reader(Cursor::new(input), "ses_test", &deadline()).unwrap();
        }
    }

    #[test]
    fn invalid_export_is_redacted_invalid_remote_stream() {
        for input in [
            format!("!{PRIVATE}"),
            format!(r#"{{"info":{{"id":"ses_test"}},"messages":[]}} {PRIVATE}"#),
            format!(r#"{{"info":{{"id":"ses_{PRIVATE}"}},"messages":[]}}"#),
            format!(r#"{{"info":{{"id":"ses_test"}},"messages":"{PRIVATE}"}}"#),
            format!(
                r#"{{"info":{{"id":"ses_test"}},"messages":[{{"info":"{PRIVATE}","parts":[]}}]}}"#
            ),
            format!(
                r#"{{"info":{{"id":"ses_test"}},"messages":[{{"info":{{}},"parts":"{PRIVATE}"}}]}}"#
            ),
            format!(r#"{{"info":{{"id":"ses_test"}},"messages":[{{"{PRIVATE}":[]}}]}}"#),
            "[]".to_owned(),
            "null".to_owned(),
        ] {
            assert_category(
                validate_export_from_reader(Cursor::new(input), "ses_test", &deadline())
                    .unwrap_err(),
                CoreError::Task(TaskErrorKind::InvalidRemoteStream),
            );
        }
    }

    #[test]
    fn decoder_local_read_errors_remain_storage_failures() {
        struct Failure;
        impl Read for Failure {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other(PRIVATE))
            }
        }
        assert_category(
            inventory_from_reader(Failure, &deadline()).unwrap_err(),
            CoreError::Task(TaskErrorKind::LocalStorage),
        );
        assert_category(
            validate_export_from_reader(Failure, "ses_test", &deadline()).unwrap_err(),
            CoreError::Task(TaskErrorKind::LocalStorage),
        );
    }

    #[test]
    fn actual_directory_decoders_validate_inventory_and_exports() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let dir = Dir::open(&root, false).unwrap();
        dir.create_new("inventory.json")
            .unwrap()
            .write_all(br#"[{"id":"ses_test"}]"#)
            .unwrap();
        dir.create_new("ses_test.json")
            .unwrap()
            .write_all(br#"{"info":{"id":"ses_test"},"messages":[]}"#)
            .unwrap();
        let ids = inventory(&dir, &deadline()).unwrap();
        let paths = vec!["ses_test.json".to_owned()];
        validate_exports(&dir, &ids, &paths, &deadline()).unwrap();
        for invalid_paths in [vec![], vec!["ses_wrong.json".to_owned()]] {
            assert_category(
                validate_exports(&dir, &ids, &invalid_paths, &deadline()).unwrap_err(),
                CoreError::Task(TaskErrorKind::InvalidRemoteStream),
            );
        }
    }

    #[test]
    fn missing_inventory_or_export_file_is_local_storage_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let dir = Dir::open(&root, false).unwrap();
        assert_category(
            inventory(&dir, &deadline()).unwrap_err(),
            CoreError::Task(TaskErrorKind::LocalStorage),
        );
        assert_category(
            validate_exports(
                &dir,
                &["ses_test".to_owned()],
                &["ses_test.json".to_owned()],
                &deadline(),
            )
            .unwrap_err(),
            CoreError::Task(TaskErrorKind::LocalStorage),
        );
    }

    #[test]
    fn literal_shell_arguments_and_exports_do_not_evaluate_roots() {
        assert_eq!(quote("a'b;$(private)"), "'a'\\''b;$(private)'");
        let script = exports(&["ses_test".to_owned()]);
        assert!(script.contains("opencode export 'ses_test'"));
        assert!(script.contains("emit 'ses_test.json'"));
        assert!(!script.contains("not-found"));
    }
}
