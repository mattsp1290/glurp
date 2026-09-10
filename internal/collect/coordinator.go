package collect

import (
	"context"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/mattsp1290/slurp/internal/archive"
	"github.com/mattsp1290/slurp/internal/config"
	"github.com/mattsp1290/slurp/internal/harness"
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
				if ctx.Err() != nil {
					itemCh <- Item{Host: host.Name, Harness: string(id), Status: "failed", Detail: "cancelled", Version: "unknown"}
					continue
				}
				taskctx := ctx
				cancel := func() {}
				if s.Timeout > 0 {
					taskctx, cancel = context.WithTimeout(ctx, s.Timeout)
				}
				batch, err := c.Archive.BeginHostHarness(host.Name, host.Destination, string(id))
				if err != nil {
					cancel()
					itemCh <- failedItem(host, id, err)
					continue
				}
				res, err := harness.Collect(taskctx, c.Runner, host, id, batch, s.Limits)
				cancel()
				if err != nil {
					batch.Abort()
					itemCh <- failedItem(host, id, err)
					continue
				}
				if res.Status == "failed" {
					batch.Abort()
					itemCh <- Item{Host: host.Name, Harness: string(id), Status: "failed", Detail: res.Detail, Version: version(res.Version), Files: res.Files, Bytes: res.Bytes}
					continue
				}
				if res.Status == "not-found" {
					batch.Abort()
					itemCh <- Item{Host: host.Name, Harness: string(id), Status: "not-found", Detail: res.Detail, Version: version(res.Version)}
					continue
				}
				batch.SetCollectorVersion(version(res.Version))
				written, unchanged, err := batch.Commit(c.Now())
				if err != nil {
					itemCh <- failedItem(host, id, err)
					continue
				}
				status := "collected"
				if written == 0 && res.Files > 0 {
					status = "unchanged"
				}
				itemCh <- Item{Host: host.Name, Harness: string(id), Status: status, Version: version(res.Version), Files: res.Files, Written: written, Unchanged: unchanged, Bytes: res.Bytes}
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
func failedItem(h config.Host, id harness.ID, err error) Item {
	return Item{Host: h.Name, Harness: string(id), Status: "failed", Detail: fmt.Sprintf("%v", err), Version: "unknown"}
}
func version(v string) string {
	if v == "" {
		return "unknown"
	}
	return v
}
