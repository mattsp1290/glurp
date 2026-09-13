package harness

import (
	"context"
	"fmt"
	"io"
	"sort"
	"strings"

	"github.com/mattsp1290/glurp/internal/archive"
	"github.com/mattsp1290/glurp/internal/config"
	"github.com/mattsp1290/glurp/internal/protocol"
	sshtransport "github.com/mattsp1290/glurp/internal/ssh"
)

type ID string

const (
	Claude   ID = "claude"
	Codex    ID = "codex"
	OpenCode ID = "opencode"
	Pi       ID = "pi"
)

var All = []ID{Claude, Codex, OpenCode, Pi}

func Parse(values []string) ([]ID, error) {
	if len(values) == 0 {
		return append([]ID(nil), All...), nil
	}
	seen := map[ID]bool{}
	out := []ID{}
	for _, v := range values {
		id := ID(v)
		ok := false
		for _, x := range All {
			if x == id {
				ok = true
			}
		}
		if !ok {
			return nil, fmt.Errorf("unknown harness %q", v)
		}
		if !seen[id] {
			seen[id] = true
			out = append(out, id)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out, nil
}

type Runner interface {
	Run(context.Context, string, string, func(io.Reader) error) (sshtransport.Result, error)
}
type Result struct {
	Status, Detail, Version string
	Files, Bytes, Unchanged uint64
}
type Limits struct{ MaxFileBytes, MaxFiles, MaxTotalBytes uint64 }

func shellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\"'\"'") + "'" }

func Collect(ctx context.Context, r Runner, host config.Host, id ID, b *archive.Batch, limits Limits) (Result, error) {
	var roots []string
	switch id {
	case Claude:
		roots = host.Sources.Claude
	case Codex:
		roots = host.Sources.Codex
	case Pi:
		roots = host.Sources.Pi
	}
	if err := b.BindRoots(roots); err != nil {
		return Result{}, err
	}
	if id == OpenCode {
		return collectOpenCode(ctx, r, host, b, limits)
	}
	script := filesystemScript(id, host)
	var pr protocol.Result
	_, err := r.Run(ctx, host.Destination, script, func(rd io.Reader) error {
		var e error
		pr, e = protocol.Decode(rd, protocol.Limits{MaxFileBytes: limits.MaxFileBytes, MaxFiles: limits.MaxFiles, MaxTotalBytes: limits.MaxTotalBytes}, func(a protocol.Artifact) error {
			u, e := b.Put(a.Path, a.Size, a.Body)
			if u {
				pr.Unchanged++
			}
			return e
		})
		return e
	})
	if err != nil {
		return Result{}, err
	}
	return Result{Status: pr.Status, Detail: pr.Detail, Version: pr.Version, Files: pr.Files, Bytes: pr.Bytes, Unchanged: pr.Unchanged}, nil
}
