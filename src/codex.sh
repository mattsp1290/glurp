#!/bin/sh
set -efu
export LC_ALL=C
printf 'GLURP1\000'
root=${CODEX_HOME:-"$HOME/.codex"}
case "$root" in /*) ;; *) exit 2;; esac
# Check all ancestors as well as the root: find does not follow descendant links.
check=$root
while :; do
    [ ! -L "$check" ] || exit 2
    [ "$check" != / ] || break
    check=${check%/*}
    [ -n "$check" ] || check=/
done
if [ ! -d "$root" ]; then printf 'E\000'; exit 0; fi
root=${root%/}
find "$root" -type f -exec sh -c '
    root=$1; shift
    for file do
        rel=${file#"$root"/}
        case "$rel" in
            sessions/*.jsonl|sessions/*.jsonl.zst|archived_sessions/*.jsonl|archived_sessions/*.jsonl.zst|session_index.jsonl) ;;
            *) continue;;
        esac
        [ -f "$file" ] && [ ! -L "$file" ] || exit 2
        before=$(cksum < "$file") || exit 2
        set -- $before
        printf "F\000%s\000%s\000" "$rel" "$2"
        cat "$file" || exit 2
        after=$(cksum < "$file") || exit 2
        [ "$before" = "$after" ] || exit 2
    done
' sh "$root" {} + || exit 2
printf 'E\000'
