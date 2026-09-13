package cli

import (
	"bytes"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mattsp1290/glurp/internal/harness"
)

func testRoot(t *testing.T) (*bytes.Buffer, Dependencies, *options) {
	t.Helper()
	out := &bytes.Buffer{}
	d := Dependencies{Stdout: out, Stderr: out, Environ: map[string]string{"HOME": t.TempDir()}, Now: func() time.Time { return time.Unix(1, 0) }}
	return out, d, &options{}
}
func TestHostCommandsAndStableJSON(t *testing.T) {
	out, d, _ := testRoot(t)
	cfg := filepath.Join(t.TempDir(), "config.json")
	for _, args := range [][]string{{"--config", cfg, "host", "add", "b", "b"}, {"--config", cfg, "host", "add", "a", "a"}} {
		root := NewRoot(d)
		root.SetArgs(args)
		if err := root.Execute(); err != nil {
			t.Fatal(err)
		}
	}
	out.Reset()
	root := NewRoot(d)
	root.SetArgs([]string{"--config", cfg, "host", "list", "--json"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	if got := out.String(); got != "[{\"name\":\"a\",\"destination\":\"a\",\"sources\":{\"claude\":[],\"codex\":[],\"pi\":[]}},{\"name\":\"b\",\"destination\":\"b\",\"sources\":{\"claude\":[],\"codex\":[],\"pi\":[]}}]\n" {
		t.Fatalf("unexpected JSON: %s", got)
	}
}
func TestSelectionFailsBeforeSSH(t *testing.T) {
	_, d, _ := testRoot(t)
	cfg := filepath.Join(t.TempDir(), "config.json")
	root := NewRoot(d)
	root.SetArgs([]string{"--config", cfg, "host", "add", "known", "dest"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	calls := 0
	d.NewRunner = func() (harness.Runner, error) { calls++; return nil, nil }
	for _, args := range [][]string{{"--config", cfg, "glurp", "missing"}, {"--config", cfg, "glurp", "--harness", "wrong"}, {"--config", cfg, "glurp", "--jobs", "0"}} {
		root = NewRoot(d)
		root.SetArgs(args)
		if err := root.Execute(); err == nil {
			t.Fatalf("expected error for %v", args)
		}
	}
	if calls != 0 {
		t.Fatalf("opened SSH %d times", calls)
	}
	root = NewRoot(d)
	root.SetArgs([]string{strings.Join([]string{"s", "lurp"}, ""), "known"})
	if err := root.Execute(); err == nil || !strings.Contains(err.Error(), "unknown command") {
		t.Fatalf("legacy collection command was accepted: %v", err)
	}
	if calls != 0 {
		t.Fatalf("legacy command opened SSH %d times", calls)
	}
}

func TestRootAndCollectionUseGlurp(t *testing.T) {
	_, d, _ := testRoot(t)
	root := NewRoot(d)
	if root.Use != "glurp" {
		t.Fatalf("root use = %q", root.Use)
	}
	cmd, _, err := root.Find([]string{"glurp"})
	if err != nil || cmd.Use != "glurp [host-name ...]" {
		t.Fatalf("collection command = %#v, %v", cmd, err)
	}
}
