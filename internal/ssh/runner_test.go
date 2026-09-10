package ssh

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestRunnerArgvAndStderrSuppression(t *testing.T) {
	dir := t.TempDir()
	capture := filepath.Join(dir, "argv")
	fake := filepath.Join(dir, "ssh")
	script := fmt.Sprintf("#!/bin/sh\nprintf '%%s\\n' \"$@\" > %s\necho SUPER_SECRET >&2\nprintf payload\n", shellLiteral(capture))
	if err := os.WriteFile(fake, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	var got string
	res, err := (Runner{Path: fake}).Run(context.Background(), "name@host", "ignored", func(r io.Reader) error { b, e := io.ReadAll(r); got = string(b); return e })
	if err != nil {
		t.Fatal(err)
	}
	if got != "payload" || res.StderrBytes == 0 {
		t.Fatalf("got=%q result=%#v", got, res)
	}
	b, _ := os.ReadFile(capture)
	args := string(b)
	if !strings.Contains(args, "name@host\n") || !strings.Contains(args, "BatchMode=yes") || strings.Contains(args, "SUPER_SECRET") {
		t.Fatalf("argv capture: %q", args)
	}
}
func TestRunnerCancellation(t *testing.T) {
	dir := t.TempDir()
	fake := filepath.Join(dir, "ssh")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\nsleep 30 &\nwait\n"), 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	start := time.Now()
	_, err := (Runner{Path: fake}).Run(ctx, "host", "", func(r io.Reader) error { _, e := io.Copy(io.Discard, r); return e })
	if err == nil {
		t.Fatal("expected cancellation error")
	}
	if time.Since(start) > 4*time.Second {
		t.Fatal("cancellation did not clean up promptly")
	}
}
func shellLiteral(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\"'\"'") + "'" }
