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

func TestExplicitRootBindingsAreAppendOnly(t *testing.T) {
	s := Store{Root: t.TempDir()}
	b, err := s.BeginHostHarness("host", "dest", "pi")
	if err != nil {
		t.Fatal(err)
	}
	if err = b.BindRoots([]string{"/one", "/two"}); err != nil {
		t.Fatal(err)
	}
	b.SetCollectorVersion("test")
	if _, _, err = b.Commit(time.Unix(1, 0)); err != nil {
		t.Fatal(err)
	}
	for name, roots := range map[string][]string{"automatic": nil, "removed": {"/one"}, "reordered": {"/two", "/one"}} {
		b, err = s.BeginHostHarness("host", "dest", "pi")
		if err != nil {
			t.Fatal(err)
		}
		if err = b.BindRoots(roots); err == nil {
			b.Abort()
			t.Errorf("accepted %s transition", name)
		} else {
			b.Abort()
		}
	}
	b, err = s.BeginHostHarness("host", "dest", "pi")
	if err != nil {
		t.Fatal(err)
	}
	if err = b.BindRoots([]string{"/one", "/two", "/three"}); err != nil {
		t.Fatalf("append rejected: %v", err)
	}
	b.Abort()
}

func TestBeginRecoversInterruptedPublication(t *testing.T) {
	root := t.TempDir()
	s := Store{Root: root}
	b, err := s.BeginHostHarness("host", "dest", "pi")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = b.Put("session.jsonl", 3, strings.NewReader("old")); err != nil {
		t.Fatal(err)
	}
	b.SetCollectorVersion("test")
	if _, _, err = b.Commit(time.Unix(1, 0)); err != nil {
		t.Fatal(err)
	}
	final := filepath.Join(root, "hosts", "host", "pi", "session.jsonl")
	stage := filepath.Join(root, ".tmp", "host", "pi", "run-crash")
	if err = os.Mkdir(stage, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.Rename(final, filepath.Join(stage, "backup-000000")); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(final, []byte("new"), 0600); err != nil {
		t.Fatal(err)
	}
	tx := transaction{Version: 1, ID: "run-crash", Stage: "run-crash", Entries: []transactionEntry{{Path: "session.jsonl", HadFinal: true}}}
	if err = atomicJSON(filepath.Join(root, "state", "host", "pi.txn.json"), tx); err != nil {
		t.Fatal(err)
	}
	b, err = s.BeginHostHarness("host", "dest", "pi")
	if err != nil {
		t.Fatal(err)
	}
	b.Abort()
	got, err := os.ReadFile(final)
	if err != nil || string(got) != "old" {
		t.Fatalf("recovery got %q: %v", got, err)
	}
	if _, err = os.Stat(filepath.Join(root, "state", "host", "pi.txn.json")); !os.IsNotExist(err) {
		t.Fatal("transaction journal remains")
	}
}
