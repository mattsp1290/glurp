package collect

import (
	"bytes"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestReportEscapesRemoteMetadata(t *testing.T) {
	r := Report{Items: []Item{{Host: "h", Harness: "pi", Status: "failed", Version: "ok\x1b]8;;bad", Detail: "line\n\x1b[31mred\u202e"}}}
	var out bytes.Buffer
	r.Print(&out)
	got := out.String()
	if strings.ContainsAny(got, "\x1b\u202e") || strings.Contains(got, "version=ok") {
		t.Fatalf("unsafe output: %q", got)
	}
	if !strings.Contains(got, "version=unknown") {
		t.Fatalf("invalid version not normalized: %q", got)
	}
}
func TestSanitizeTruncatesOnUTF8Boundary(t *testing.T) {
	got := sanitize(strings.Repeat("a", 239) + "étail")
	if !utf8.ValidString(got) || len(got) > 240 {
		t.Fatalf("invalid truncation: %q", got)
	}
}
func TestNormalizeVersion(t *testing.T) {
	for _, v := range []string{"codex-cli 0.154.0", "2.1.268 (Claude Code)", "opencode/1.2.3"} {
		if normalizeVersion(v) != v {
			t.Errorf("rejected %q", v)
		}
	}
	for _, v := range []string{"", strings.Repeat("a", 129), "secret\nsecond", "x\x1b[2J"} {
		if normalizeVersion(v) != "unknown" {
			t.Errorf("accepted %q", v)
		}
	}
}
