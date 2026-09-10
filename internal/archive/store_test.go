package archive

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestAtomicIdempotentAndIdentity(t *testing.T) {
	root := t.TempDir()
	s := Store{Root: root}
	b, err := s.BeginHostHarness("host", "dest", "claude")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = b.Put("project/a.jsonl", 5, strings.NewReader("hello")); err != nil {
		t.Fatal(err)
	}
	w, u, err := b.Commit(time.Unix(1, 0))
	if err != nil || w != 1 || u != 0 {
		t.Fatalf("commit %d %d %v", w, u, err)
	}
	p := filepath.Join(root, "hosts", "host", "claude", "project", "a.jsonl")
	st, _ := os.Stat(p)
	mtime := st.ModTime()
	b, _ = s.BeginHostHarness("host", "dest", "claude")
	same, err := b.Put("project/a.jsonl", 5, strings.NewReader("hello"))
	if err != nil || !same {
		t.Fatalf("unchanged=%v err=%v", same, err)
	}
	w, u, err = b.Commit(time.Unix(2, 0))
	if err != nil || w != 0 || u != 1 {
		t.Fatal(err)
	}
	st, _ = os.Stat(p)
	if !st.ModTime().Equal(mtime) {
		t.Fatal("unchanged artifact was rewritten")
	}
	if _, err = s.BeginHostHarness("host", "other", "claude"); err == nil {
		t.Fatal("expected identity conflict")
	}
}
func TestTraversalAndAbort(t *testing.T) {
	root := t.TempDir()
	b, err := (Store{Root: root}).BeginHostHarness("host", "dest", "pi")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = b.Put("../escape", 1, strings.NewReader("x")); err == nil {
		t.Fatal("expected traversal rejection")
	}
	if _, err = b.Put("short.jsonl", 10, strings.NewReader("short")); err == nil {
		t.Fatal("expected interrupted body error")
	}
	b.Abort()
	if _, err := os.Stat(filepath.Join(root, "hosts", "host", "pi", "escape")); !os.IsNotExist(err) {
		t.Fatal("partial final artifact exists")
	}
}
