package harness

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/mattsp1290/slurp/internal/archive"
	"github.com/mattsp1290/slurp/internal/config"
	sshtransport "github.com/mattsp1290/slurp/internal/ssh"
)

type shellRunner struct{ home string }

func (r shellRunner) Run(ctx context.Context, _ string, script string, consume func(io.Reader) error) (sshtransport.Result, error) {
	cmd := exec.CommandContext(ctx, "/bin/sh")
	cmd.Dir = r.home
	cmd.Env = []string{"HOME=" + r.home, "PATH=" + os.Getenv("PATH"), "TMPDIR=" + os.TempDir()}
	cmd.Stdin = io.NopCloser(stringsReader(script))
	out, e := cmd.StdoutPipe()
	if e != nil {
		return sshtransport.Result{}, e
	}
	cmd.Stderr = io.Discard
	if e = cmd.Start(); e != nil {
		return sshtransport.Result{}, e
	}
	ce := consume(out)
	we := cmd.Wait()
	if ce != nil {
		return sshtransport.Result{}, ce
	}
	return sshtransport.Result{}, we
}

func TestExplicitRootIsLiteralAndRequired(t *testing.T) {
	home := t.TempDir()
	root := filepath.Join(home, "odd'; touch PWNED; echo '")
	write(t, filepath.Join(root, "a.jsonl"), "safe\n")
	data := t.TempDir()
	host := config.Host{Name: "fixture", Destination: "fixture", Sources: config.SourceOverrides{Pi: []string{root}}}
	if err := config.ValidateHost(host); err != nil {
		t.Fatal(err)
	}
	b, err := (archive.Store{Root: data}).BeginHostHarness("fixture", "fixture", "pi")
	if err != nil {
		t.Fatal(err)
	}
	res, err := Collect(context.Background(), shellRunner{home}, host, Pi, b, Limits{MaxFileBytes: 1024, MaxFiles: 10, MaxTotalBytes: 4096})
	if err != nil {
		b.Abort()
		t.Fatal(err)
	}
	if res.Status != "ok" || res.Files != 1 {
		t.Fatalf("result %#v", res)
	}
	if _, _, err = b.Commit(time.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(home, "PWNED")); !os.IsNotExist(err) {
		t.Fatal("source root executed shell content")
	}
	host.Sources.Pi = []string{filepath.Join(home, "missing")}
	b, err = (archive.Store{Root: t.TempDir()}).BeginHostHarness("fixture", "fixture", "pi")
	if err != nil {
		t.Fatal(err)
	}
	res, err = Collect(context.Background(), shellRunner{home}, host, Pi, b, Limits{MaxFileBytes: 1024, MaxFiles: 10, MaxTotalBytes: 4096})
	if err != nil {
		b.Abort()
		t.Fatal(err)
	}
	b.Abort()
	if res.Status != "failed" {
		t.Fatalf("missing explicit root was %#v", res)
	}
}

type sreader struct {
	s string
	i int
}

func stringsReader(s string) *sreader { return &sreader{s: s} }
func (r *sreader) Read(p []byte) (int, error) {
	if r.i == len(r.s) {
		return 0, io.EOF
	}
	n := copy(p, r.s[r.i:])
	r.i += n
	return n, nil
}
func write(t *testing.T, p, s string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(p), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(s), 0600); err != nil {
		t.Fatal(err)
	}
}
func TestNativeCollectorsUnderBinSh(t *testing.T) {
	home := t.TempDir()
	write(t, filepath.Join(home, ".claude/projects/proj/a.jsonl"), "claude\n")
	write(t, filepath.Join(home, ".claude/projects/proj/no.md"), "no")
	write(t, filepath.Join(home, ".codex/sessions/2026/c.jsonl"), "codex\n")
	write(t, filepath.Join(home, ".codex/archived_sessions/z.jsonl.zst"), "zstd")
	write(t, filepath.Join(home, ".codex/session_index.jsonl"), "index\n")
	write(t, filepath.Join(home, ".pi/agent/sessions/work/p.jsonl"), "pi\n")
	root := t.TempDir()
	for _, id := range []ID{Claude, Codex, Pi} {
		b, e := (archive.Store{Root: root}).BeginHostHarness("fixture", "fixture", string(id))
		if e != nil {
			t.Fatal(e)
		}
		res, e := Collect(context.Background(), shellRunner{home}, config.Host{Name: "fixture", Destination: "fixture"}, id, b, Limits{MaxFileBytes: 1024, MaxFiles: 20, MaxTotalBytes: 4096})
		if e != nil {
			b.Abort()
			t.Fatalf("%s: %v", id, e)
		}
		if res.Status != "ok" || res.Files == 0 {
			t.Fatalf("%s: %#v", id, res)
		}
		if _, _, e = b.Commit(time.Now()); e != nil {
			t.Fatal(e)
		}
	}
	for _, p := range []string{"claude/proj/a.jsonl", "codex/sessions/2026/c.jsonl", "codex/archived_sessions/z.jsonl.zst", "pi/work/p.jsonl"} {
		if _, e := os.Stat(filepath.Join(root, "hosts", "fixture", p)); e != nil {
			t.Errorf("missing %s: %v", p, e)
		}
	}
}
