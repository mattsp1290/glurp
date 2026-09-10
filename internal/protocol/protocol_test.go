package protocol

import (
	"bytes"
	"fmt"
	"io"
	"testing"
)

func stream(path, body, status string) []byte {
	var b bytes.Buffer
	b.WriteString(Magic)
	fmt.Fprintf(&b, "F\x00%s\x00%d\x001\x00", path, len(body))
	b.WriteString(body)
	fmt.Fprintf(&b, "T\x00%s\x00\x00v\x00E\x00", status)
	return b.Bytes()
}
func TestDecode(t *testing.T) {
	var got string
	r, err := Decode(bytes.NewReader(stream("a/b.jsonl", "hello", "ok")), Limits{MaxFileBytes: 5, MaxFiles: 1, MaxTotalBytes: 5}, func(a Artifact) error { b, e := io.ReadAll(a.Body); got = string(b); return e })
	if err != nil || got != "hello" || r.Files != 1 {
		t.Fatalf("result=%#v got=%q err=%v", r, got, err)
	}
}
func TestDecodeRejectsUnsafeAndTruncated(t *testing.T) {
	for _, in := range [][]byte{stream("../escape", "x", "ok"), stream("a", "xx", "ok")[:len(stream("a", "xx", "ok"))-2]} {
		_, err := Decode(bytes.NewReader(in), Limits{MaxFileBytes: 10, MaxFiles: 2, MaxTotalBytes: 20}, func(a Artifact) error {
			if e := ValidateRelativePath(a.Path); e != nil {
				return e
			}
			_, e := io.Copy(io.Discard, a.Body)
			return e
		})
		if err == nil {
			t.Fatal("expected rejection")
		}
	}
}
func FuzzDecode(f *testing.F) {
	f.Add(stream("a", "x", "ok"))
	f.Fuzz(func(t *testing.T, b []byte) {
		_, _ = Decode(bytes.NewReader(b), Limits{MaxFileBytes: 1024, MaxFiles: 20, MaxTotalBytes: 4096}, func(a Artifact) error {
			if err := ValidateRelativePath(a.Path); err != nil {
				return err
			}
			_, err := io.Copy(io.Discard, a.Body)
			return err
		})
	})
}
