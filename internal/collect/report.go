package collect

import (
	"fmt"
	"io"
	"sort"
	"strings"
)

type Item struct {
	Host, Harness, Status, Detail, Version string
	Files, Written, Unchanged, Bytes       uint64
}
type Report struct{ Items []Item }

func (r Report) Failed() bool {
	for _, i := range r.Items {
		if i.Status == "failed" {
			return true
		}
	}
	return false
}
func (r Report) Print(w io.Writer) {
	items := append([]Item(nil), r.Items...)
	sort.Slice(items, func(i, j int) bool {
		if items[i].Host == items[j].Host {
			return items[i].Harness < items[j].Harness
		}
		return items[i].Host < items[j].Host
	})
	var files, written, unchanged, bytes, skipped, failed uint64
	for _, i := range items {
		detail := ""
		if i.Detail != "" {
			detail = " (" + sanitize(i.Detail) + ")"
		}
		fmt.Fprintf(w, "%s/%s: %s; files=%d written=%d unchanged=%d bytes=%d version=%s%s\n", i.Host, i.Harness, i.Status, i.Files, i.Written, i.Unchanged, i.Bytes, i.Version, detail)
		files += i.Files
		written += i.Written
		unchanged += i.Unchanged
		bytes += i.Bytes
		if i.Status == "not-found" {
			skipped++
		}
		if i.Status == "failed" {
			failed++
		}
	}
	fmt.Fprintf(w, "total: files=%d written=%d unchanged=%d bytes=%d skipped=%d failures=%d\n", files, written, unchanged, bytes, skipped, failed)
}
func sanitize(s string) string {
	s = strings.Map(func(r rune) rune {
		if r < ' ' || r == 127 {
			return -1
		}
		return r
	}, s)
	if len(s) > 240 {
		return s[:240]
	}
	return s
}
