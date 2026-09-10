package cli

import (
	"bytes"
	"path/filepath"
	"testing"
	"time"

	"github.com/mattsp1290/slurp/internal/harness"
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
	for _, args := range [][]string{{"--config", cfg, "slurp", "missing"}, {"--config", cfg, "slurp", "--harness", "wrong"}, {"--config", cfg, "slurp", "--jobs", "0"}} {
		root = NewRoot(d)
		root.SetArgs(args)
		if err := root.Execute(); err == nil {
			t.Fatalf("expected error for %v", args)
		}
	}
	if calls != 0 {
		t.Fatalf("opened SSH %d times", calls)
	}
}
