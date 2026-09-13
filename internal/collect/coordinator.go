package collect

import (
	"context"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/mattsp1290/glurp/internal/archive"
	"github.com/mattsp1290/glurp/internal/config"
	"github.com/mattsp1290/glurp/internal/harness"
)

type Selection struct {
	Hosts     []config.Host
	Harnesses []harness.ID
	Jobs      int
	Timeout   time.Duration
	Limits    harness.Limits
}
type Coordinator struct {
	Runner  harness.Runner
	Archive archive.Store
	Now     func() time.Time
}

func (c Coordinator) Run(ctx context.Context, s Selection) Report {
	if len(s.Hosts) == 0 {
		return Report{}
	}
	if c.Now == nil {
		c.Now = time.Now
	}
	jobs := s.Jobs
	if jobs < 1 {
		jobs = 1
	}
	hostCh := make(chan config.Host)
	itemCh := make(chan Item)
	var wg sync.WaitGroup
	worker := func() {
		defer wg.Done()
		for host := range hostCh {
			for _, id := range s.Harnesses {
				itemCh <- c.runOne(ctx, s, host, id)
			}
		}
	}
	if jobs > len(s.Hosts) {
		jobs = len(s.Hosts)
	}
	for i := 0; i < jobs; i++ {
		wg.Add(1)
		go worker()
	}
	go func() {
		defer close(hostCh)
		for _, h := range s.Hosts {
			hostCh <- h
		}
	}()
	go func() { wg.Wait(); close(itemCh) }()
	r := Report{}
	for i := range itemCh {
		r.Items = append(r.Items, i)
	}
	sort.Slice(r.Items, func(i, j int) bool {
		if r.Items[i].Host == r.Items[j].Host {
			return r.Items[i].Harness < r.Items[j].Harness
		}
		return r.Items[i].Host < r.Items[j].Host
	})
	return r
}

func (c Coordinator) runOne(ctx context.Context, s Selection, host config.Host, id harness.ID) Item {
	if ctx.Err() != nil {
		return Item{Host: host.Name, Harness: string(id), Status: "failed", Detail: "cancelled", Version: "unknown"}
	}
	taskctx := ctx
	cancel := func() {}
	if s.Timeout > 0 {
		taskctx, cancel = context.WithTimeout(ctx, s.Timeout)
	}
	defer cancel()
	batch, err := c.Archive.BeginHostHarness(host.Name, host.Destination, string(id))
	if err != nil {
		return failedItem(host, id, err)
	}
	defer batch.Abort()
	res, err := harness.Collect(taskctx, c.Runner, host, id, batch, s.Limits)
	if err != nil {
		return failedItem(host, id, err)
	}
	if res.Status == "failed" {
		return Item{Host: host.Name, Harness: string(id), Status: "failed", Detail: res.Detail, Version: version(res.Version), Files: res.Files, Bytes: res.Bytes}
	}
	if res.Status == "not-found" {
		return Item{Host: host.Name, Harness: string(id), Status: "not-found", Detail: res.Detail, Version: version(res.Version)}
	}
	batch.SetCollectorVersion(version(res.Version))
	written, unchanged, err := batch.Commit(c.Now())
	if err != nil {
		return failedItem(host, id, err)
	}
	status := "collected"
	if written == 0 && res.Files > 0 {
		status = "unchanged"
	}
	return Item{Host: host.Name, Harness: string(id), Status: status, Version: version(res.Version), Files: res.Files, Written: written, Unchanged: unchanged, Bytes: res.Bytes}
}
func failedItem(h config.Host, id harness.ID, err error) Item {
	return Item{Host: h.Name, Harness: string(id), Status: "failed", Detail: fmt.Sprintf("%v", err), Version: "unknown"}
}
func version(v string) string {
	return normalizeVersion(v)
}
