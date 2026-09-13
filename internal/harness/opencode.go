package harness

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"regexp"
	"sort"
	"strings"

	"github.com/mattsp1290/glurp/internal/archive"
	"github.com/mattsp1290/glurp/internal/config"
)

var sessionID = regexp.MustCompile(`^ses_[A-Za-z0-9_-]+$`)

func collectOpenCode(ctx context.Context, r Runner, host config.Host, b *archive.Batch, limits Limits) (Result, error) {
	const maxList = 64 << 20
	var raw []byte
	version := "unknown"
	_, _ = r.Run(ctx, host.Destination, "#!/bin/sh\nexec opencode --version\n", func(rd io.Reader) error {
		var buf bytes.Buffer
		n, e := io.Copy(&buf, io.LimitReader(rd, 4097))
		if e != nil {
			return e
		}
		if n > 4096 {
			return fmt.Errorf("OpenCode version output exceeds limit")
		}
		if buf.Len() <= 4096 {
			if v := strings.TrimSpace(buf.String()); v != "" && !strings.ContainsAny(v, "\r\n") {
				version = v
			}
		}
		return nil
	})
	script := `#!/bin/sh
platform=$(uname -s 2>/dev/null || printf unknown)
case "$platform" in Linux|Darwin) ;; *) printf 'GLURP_UNSUPPORTED_PLATFORM\n'; exit 0;; esac
if ! command -v opencode >/dev/null 2>&1; then printf 'GLURP_OPENCODE_NOT_FOUND\n'; exit 0; fi
exec opencode db 'SELECT id FROM session ORDER BY id' --format json
`
	_, err := r.Run(ctx, host.Destination, script, func(rd io.Reader) error {
		var buf bytes.Buffer
		n, e := io.Copy(&buf, io.LimitReader(rd, maxList+1))
		if e != nil {
			return e
		}
		if n > maxList {
			return fmt.Errorf("OpenCode session inventory exceeds limit")
		}
		raw = buf.Bytes()
		return nil
	})
	if err != nil {
		return Result{}, fmt.Errorf("OpenCode inventory failed: %w", err)
	}
	if string(raw) == "GLURP_OPENCODE_NOT_FOUND\n" {
		return Result{Status: "not-found", Version: "unknown"}, nil
	}
	if string(raw) == "GLURP_UNSUPPORTED_PLATFORM\n" {
		return Result{}, fmt.Errorf("unsupported remote platform")
	}
	var rows []struct {
		ID string `json:"id"`
	}
	d := json.NewDecoder(bytes.NewReader(raw))
	if err := d.Decode(&rows); err != nil {
		return Result{}, fmt.Errorf("OpenCode database schema returned invalid JSON")
	}
	if rows == nil {
		return Result{}, fmt.Errorf("OpenCode database schema did not return an array")
	}
	if d.Decode(&struct{}{}) != io.EOF {
		return Result{}, fmt.Errorf("OpenCode database schema returned trailing JSON")
	}
	ids := make([]string, 0, len(rows))
	seen := map[string]bool{}
	for _, row := range rows {
		if !sessionID.MatchString(row.ID) || seen[row.ID] {
			return Result{}, fmt.Errorf("OpenCode database returned invalid or duplicate session ID")
		}
		seen[row.ID] = true
		ids = append(ids, row.ID)
	}
	sort.Strings(ids)
	var out Result
	out.Status = "ok"
	out.Version = version
	for _, id := range ids {
		if limits.MaxFiles > 0 && out.Files >= limits.MaxFiles {
			return Result{}, fmt.Errorf("artifact count limit exceeded")
		}
		var unchanged bool
		var size uint64
		artifactLimit := limits.MaxFileBytes
		if limits.MaxTotalBytes > 0 {
			remaining := limits.MaxTotalBytes - out.Bytes
			if remaining < artifactLimit {
				artifactLimit = remaining
			}
		}
		script = "#!/bin/sh\nexec opencode export " + shellQuote(id) + "\n"
		_, err = r.Run(ctx, host.Destination, script, func(rd io.Reader) error {
			var e error
			unchanged, size, e = b.PutJSONStream("sessions/"+id+".json", artifactLimit, rd)
			return e
		})
		if err != nil {
			return Result{}, fmt.Errorf("OpenCode export %s failed: %w", id, err)
		}
		out.Files++
		if ^uint64(0)-out.Bytes < size {
			return Result{}, fmt.Errorf("total byte limit exceeded")
		}
		out.Bytes += size
		if unchanged {
			out.Unchanged++
		}
		if limits.MaxTotalBytes > 0 && out.Bytes > limits.MaxTotalBytes {
			return Result{}, fmt.Errorf("total byte limit exceeded")
		}
	}
	return out, nil
}
