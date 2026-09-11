package harness

import (
	"fmt"
	"strings"

	"github.com/mattsp1290/slurp/internal/config"
)

type filesystemHarness struct {
	roots   []string
	auto    string
	command string
	match   string
	logical string
}

func describeFilesystemHarness(id ID, host config.Host) filesystemHarness {
	switch id {
	case Claude:
		return filesystemHarness{
			roots:   host.Sources.Claude,
			command: "claude",
			auto:    `if [ -n "${CLAUDE_CONFIG_DIR:-}" ] && [ "${CLAUDE_CONFIG_DIR#/}" != "$CLAUDE_CONFIG_DIR" ]; then root=$CLAUDE_CONFIG_DIR/projects; else root=$HOME/.claude/projects; fi`,
			match:   "*.jsonl",
			logical: "plain",
		}
	case Codex:
		return filesystemHarness{
			roots:   host.Sources.Codex,
			command: "codex",
			auto:    `if [ -n "${CODEX_HOME:-}" ] && [ "${CODEX_HOME#/}" != "$CODEX_HOME" ]; then root=$CODEX_HOME; else root=$HOME/.codex; fi`,
			match:   "codex",
			logical: "codex",
		}
	case Pi:
		return filesystemHarness{
			roots:   host.Sources.Pi,
			command: "pi",
			auto:    `if [ -n "${PI_CODING_AGENT_SESSION_DIR:-}" ] && [ "${PI_CODING_AGENT_SESSION_DIR#/}" != "$PI_CODING_AGENT_SESSION_DIR" ]; then root=$PI_CODING_AGENT_SESSION_DIR; elif [ -n "${PI_CODING_AGENT_DIR:-}" ] && [ "${PI_CODING_AGENT_DIR#/}" != "$PI_CODING_AGENT_DIR" ]; then root=$PI_CODING_AGENT_DIR/sessions; else root=$HOME/.pi/agent/sessions; fi`,
			match:   "*.jsonl",
			logical: "plain",
		}
	}
	panic("unsupported filesystem harness: " + id)
}

func filesystemScript(id ID, host config.Host) string {
	d := describeFilesystemHarness(id, host)
	var calls strings.Builder
	if len(d.roots) > 0 {
		for i, p := range d.roots {
			calls.WriteString(fmt.Sprintf("collect_root %s %s || fail=1\n", shellQuote(p), shellQuote(fmt.Sprintf("root-%04d/", i+1))))
		}
	} else {
		calls.WriteString(d.auto + "\n")
		if id == Codex {
			calls.WriteString("if { [ -d \"$root/sessions\" ] && [ ! -L \"$root/sessions\" ]; } || { [ -d \"$root/archived_sessions\" ] && [ ! -L \"$root/archived_sessions\" ]; } || { [ -f \"$root/session_index.jsonl\" ] && [ ! -L \"$root/session_index.jsonl\" ]; }; then collect_root \"$root\" '' || fail=1; fi\n")
		} else {
			calls.WriteString("if [ -d \"$root\" ]; then collect_root \"$root\" '' || fail=1; fi\n")
		}
	}
	return fmt.Sprintf(`#!/bin/sh
set -f
printf 'SLURP\000\001\000'
platform=$(uname -s 2>/dev/null || printf unknown)
case "$platform" in Linux|Darwin) ;; *) printf 'T\000failed\000unsupported remote platform\000unknown\000E\000'; exit 0;; esac
if command -v %s >/dev/null 2>&1; then installed=1; version=$(%s --version 2>/dev/null | sed -n '1p'); else installed=0; version=unknown; fi
[ -n "$version" ] || version=unknown
fail=0
found=0
collect_root() {
 root=$1
 prefix=$2
 case "$root" in '~/'*) root=$HOME/${root#\~/};; /*) ;; *) return 2;; esac
 while [ "$root" != / ] && [ "${root%%/}" != "$root" ]; do root=${root%%/}; done
 if [ ! -d "$root" ] || [ ! -r "$root" ] || [ -L "$root" ]; then return 2; fi
 found=1
 find "$root" -type f -exec sh -c '
   root=$1; prefix=$2; kind=$3; pattern=$4; version=$5; shift 5
   for file do
     rel=${file#"$root"/}
     case "$kind" in
      codex) case "$rel" in sessions/*.jsonl|sessions/*.jsonl.zst|archived_sessions/*.jsonl|archived_sessions/*.jsonl.zst|session_index.jsonl) ;; *) continue;; esac ;;
      *) case "$rel" in $pattern) ;; *) continue;; esac ;;
     esac
     if [ ! -f "$file" ] || [ -L "$file" ]; then printf "T\000failed\000source entry changed during collection\000%%s\000E\000" "$version"; exit 3; fi
     before=$(cksum "$file" 2>/dev/null) || { printf "T\000failed\000source read failed\000%%s\000E\000" "$version"; exit 3; }
     set -- $before; sum=$1; size=$2
     printf "F\000%%s\000%%s\000%%s\000" "$prefix$rel" "$size" "$sum"
     cat "$file" || { printf "T\000failed\000source read failed\000%%s\000E\000" "$version"; exit 3; }
     after=$(cksum "$file" 2>/dev/null) || { printf "T\000failed\000source stability check failed\000%%s\000E\000" "$version"; exit 3; }
     [ "$before" = "$after" ] || { printf "T\000failed\000source changed during collection\000%%s\000E\000" "$version"; exit 3; }
   done
 ' sh "$root" "$prefix" %s %s "$version" {} + || return 3
}
%s
if [ "$fail" -ne 0 ]; then printf 'T\000failed\000source missing, unreadable, or changed during collection\000%%s\000E\000' "$version"; exit 0; fi
if [ "$found" -eq 0 ]; then
 if [ %d -ne 0 ] || [ "$installed" -eq 1 ]; then printf 'T\000failed\000source location unresolved; configure an explicit path\000%%s\000E\000' "$version"; else printf 'T\000not-found\000source not found\000unknown\000E\000'; fi
 exit 0
fi
printf 'T\000ok\000\000%%s\000E\000' "$version"
	`, shellQuote(d.command), shellQuote(d.command), shellQuote(d.logical), shellQuote(d.match), calls.String(), len(d.roots))
}
