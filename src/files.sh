set -efu
export LC_ALL=C
printf 'GLURP1\000'
found=0
index=0
for root do
    index=$((index + 1))
    case "$root" in '~/'*) root=$HOME/${root#\~/};; esac
    case "$root" in /*) ;; *) exit 2;; esac
    root=${root%/}; [ -n "$root" ] || root=/
    check=$root
    while :; do
        [ ! -L "$check" ] || exit 2
        [ "$check" != / ] || break
        check=${check%/*}; [ -n "$check" ] || check=/
    done
    if [ ! -d "$root" ]; then
        [ "$required" = 0 ] || exit 2
        command -v "$executable" >/dev/null 2>&1 && exit 2
        continue
    fi
    [ -r "$root" ] && [ -x "$root" ] || exit 2
    found=1
    prefix=''
    [ "$required" = 0 ] || prefix=$(printf 'root-%04d/' "$index")
    find "$root" -type f -exec sh -c '
        root=$1; prefix=$2; harness=$3; shift 3
        for file do
            rel=${file#"$root"/}
            case "$harness:$rel" in
                codex:sessions/*.jsonl|codex:sessions/*.jsonl.zst|codex:archived_sessions/*.jsonl|codex:archived_sessions/*.jsonl.zst|codex:session_index.jsonl|claude:projects/*.jsonl|pi:*.jsonl) ;;
                *) continue;;
            esac
            [ -f "$file" ] && [ ! -L "$file" ] || exit 2
            before=$(cksum < "$file") || exit 2
            set -- $before
            printf "F\000%s%s\000%s\000" "$prefix" "$rel" "$2"
            cat "$file" || exit 2
            after=$(cksum < "$file") || exit 2
            [ "$before" = "$after" ] || exit 2
        done
    ' sh "$root" "$prefix" "$harness" {} + || exit 2
done
if [ "$found" = 1 ]; then printf 'T\000ok\000'; else printf 'T\000not-found\000'; fi
printf 'E\000'
