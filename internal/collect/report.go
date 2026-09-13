package collect

import (
	"fmt"
	"io"
	"regexp"
	"sort"
	"strings"
	"unicode"
	"unicode/utf8"
)

type Item struct {
	Host, Harness, Status, Detail, Version string
	Files, Written, Unchanged, Bytes       uint64
}
type Report struct{ Items []Item }

var safeVersion = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._+ /@():-]{0,127}$`)

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
		fmt.Fprintf(w, "%s/%s: %s; files=%d written=%d unchanged=%d bytes=%d version=%s%s\n", i.Host, i.Harness, i.Status, i.Files, i.Written, i.Unchanged, i.Bytes, normalizeVersion(i.Version), detail)
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
		if unicode.IsControl(r) || r >= 0x202a && r <= 0x202e || r >= 0x2066 && r <= 0x2069 {
			return -1
		}
		return r
	}, s)
	if len(s) > 240 {
		end := 240
		for end > 0 && !utf8.RuneStart(s[end]) {
			end--
		}
		return s[:end]
	}
	return s
}

func normalizeVersion(v string) string {
	v = strings.TrimSpace(v)
	if !safeVersion.MatchString(v) {
		return "unknown"
	}
	return v
}
