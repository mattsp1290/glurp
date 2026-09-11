package harness

import (
	"bytes"
	"context"
	"io"
	"strings"
	"testing"

	"github.com/mattsp1290/slurp/internal/archive"
	"github.com/mattsp1290/slurp/internal/config"
	sshtransport "github.com/mattsp1290/slurp/internal/ssh"
)

type versionFloodRunner struct{ calls int }

func (r *versionFloodRunner) Run(_ context.Context, _ string, _ string, consume func(io.Reader) error) (sshtransport.Result, error) {
	r.calls++
	if r.calls == 1 {
		return sshtransport.Result{}, consume(strings.NewReader(strings.Repeat("v", 1<<20)))
	}
	return sshtransport.Result{}, consume(bytes.NewBufferString("[]"))
}
func TestOpenCodeOversizedVersionIsBestEffort(t *testing.T) {
	runner := &versionFloodRunner{}
	b, err := (archive.Store{Root: t.TempDir()}).BeginHostHarness("h", "d", "opencode")
	if err != nil {
		t.Fatal(err)
	}
	defer b.Abort()
	res, err := Collect(context.Background(), runner, config.Host{Name: "h", Destination: "d"}, OpenCode, b, Limits{MaxFileBytes: 1024, MaxFiles: 10, MaxTotalBytes: 4096})
	if err != nil {
		t.Fatal(err)
	}
	if res.Status != "ok" || res.Version != "unknown" || runner.calls != 2 {
		t.Fatalf("unexpected result %#v calls=%d", res, runner.calls)
	}
}
