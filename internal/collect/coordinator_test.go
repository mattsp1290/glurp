package collect

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/mattsp1290/slurp/internal/archive"
	"github.com/mattsp1290/slurp/internal/config"
	"github.com/mattsp1290/slurp/internal/harness"
	"github.com/mattsp1290/slurp/internal/protocol"
	sshtransport "github.com/mattsp1290/slurp/internal/ssh"
)

type trackingRunner struct {
	active, max     atomic.Int32
	mu              sync.Mutex
	perHost         map[string]int
	sameHostOverlap bool
}

func (r *trackingRunner) Run(ctx context.Context, destination, script string, consume func(io.Reader) error) (sshtransport.Result, error) {
	if destination == "bad" {
		return sshtransport.Result{ExitCode: 255}, fmt.Errorf("transport failed")
	}
	now := r.active.Add(1)
	for {
		old := r.max.Load()
		if now <= old || r.max.CompareAndSwap(old, now) {
			break
		}
	}
	r.mu.Lock()
	r.perHost[destination]++
	if r.perHost[destination] > 1 {
		r.sameHostOverlap = true
	}
	r.mu.Unlock()
	defer func() { r.mu.Lock(); r.perHost[destination]--; r.mu.Unlock(); r.active.Add(-1) }()
	select {
	case <-ctx.Done():
		return sshtransport.Result{}, ctx.Err()
	case <-time.After(10 * time.Millisecond):
	}
	payload := []byte(protocol.Magic + "T\x00not-found\x00none\x00unknown\x00E\x00")
	return sshtransport.Result{}, consume(bytes.NewReader(payload))
}

func TestCoordinatorBoundsAndSerializesWork(t *testing.T) {
	runner := &trackingRunner{perHost: map[string]int{}}
	hosts := []config.Host{{Name: "b", Destination: "b"}, {Name: "a", Destination: "a"}, {Name: "bad", Destination: "bad"}}
	c := Coordinator{Runner: runner, Archive: archive.Store{Root: t.TempDir()}, Now: func() time.Time { return time.Unix(1, 0) }}
	r := c.Run(context.Background(), Selection{Hosts: hosts, Harnesses: []harness.ID{harness.Claude, harness.Codex, harness.Pi}, Jobs: 2, Timeout: time.Second, Limits: harness.Limits{MaxFileBytes: 1024, MaxFiles: 10, MaxTotalBytes: 4096}})
	if runner.max.Load() > 2 || runner.max.Load() < 2 {
		t.Fatalf("unexpected concurrency %d", runner.max.Load())
	}
	if runner.sameHostOverlap {
		t.Fatal("same host ran concurrently")
	}
	if len(r.Items) != 9 || !r.Failed() {
		t.Fatalf("unexpected report %#v", r)
	}
	for i := 1; i < len(r.Items); i++ {
		prev, cur := r.Items[i-1], r.Items[i]
		if prev.Host > cur.Host || prev.Host == cur.Host && prev.Harness > cur.Harness {
			t.Fatal("report is not sorted")
		}
	}
}
func TestRunOneHonorsCancelledContext(t *testing.T) {
	runner := &trackingRunner{perHost: map[string]int{}}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	c := Coordinator{Runner: runner, Archive: archive.Store{Root: t.TempDir()}, Now: time.Now}
	item := c.runOne(ctx, Selection{}, config.Host{Name: "a", Destination: "a"}, harness.Claude)
	if item.Status != "failed" || runner.active.Load() != 0 {
		t.Fatalf("unexpected item %#v", item)
	}
}
