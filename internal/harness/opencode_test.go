package harness

import (
	"bytes"
	"context"
	"io"
	"strings"
	"testing"

	"github.com/mattsp1290/glurp/internal/archive"
	"github.com/mattsp1290/glurp/internal/config"
	sshtransport "github.com/mattsp1290/glurp/internal/ssh"
)

type versionFloodRunner struct{ calls int }

func (r *versionFloodRunner) Run(_ context.Context, _ string, _ string, consume func(io.Reader) error) (sshtransport.Result, error) {
	r.calls++
	if r.calls == 1 {
		return sshtransport.Result{}, consume(strings.NewReader(strings.Repeat("v", 1<<20)))
	}
	return sshtransport.Result{}, consume(bytes.NewBufferString("[]"))
}

type sentinelRunner struct{ inventory string }

func (r sentinelRunner) Run(_ context.Context, _ string, _ string, consume func(io.Reader) error) (sshtransport.Result, error) {
	return sshtransport.Result{}, consume(strings.NewReader(r.inventory))
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

func TestOpenCodeGlurpSentinels(t *testing.T) {
	host := config.Host{Name: "h", Destination: "d"}
	for _, tc := range []struct {
		name, inventory string
		wantErr         bool
	}{
		{name: "not found", inventory: "GLURP_OPENCODE_NOT_FOUND\n"},
		{name: "unsupported platform", inventory: "GLURP_UNSUPPORTED_PLATFORM\n", wantErr: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			b, err := (archive.Store{Root: t.TempDir()}).BeginHostHarness("h", "d", "opencode")
			if err != nil {
				t.Fatal(err)
			}
			defer b.Abort()
			res, err := Collect(context.Background(), sentinelRunner{inventory: tc.inventory}, host, OpenCode, b, Limits{})
			if tc.wantErr {
				if err == nil {
					t.Fatal("expected unsupported platform error")
				}
				return
			}
			if err != nil || res.Status != "not-found" {
				t.Fatalf("result %#v, error %v", res, err)
			}
		})
	}
}
