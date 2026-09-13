package config

import (
	"os"
	"path/filepath"
	"sync"
	"testing"
)

func TestStoreCRUDAndPermissions(t *testing.T) {
	root := t.TempDir()
	if err := os.Chmod(root, 0700); err != nil {
		t.Fatal(err)
	}
	p := filepath.Join(root, "config", "config.json")
	s := Store{Path: p}
	var wg sync.WaitGroup
	for _, n := range []string{"b", "a"} {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := s.AddHost(Host{Name: n, Destination: n}); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	c, err := s.Load()
	if err != nil {
		t.Fatal(err)
	}
	if len(c.Hosts) != 2 || c.Hosts[0].Name != "a" {
		t.Fatalf("bad config: %#v", c)
	}
	st, _ := os.Stat(p)
	if st.Mode().Perm() != 0600 {
		t.Fatalf("mode %o", st.Mode().Perm())
	}
	if err := s.RemoveHost("a"); err != nil {
		t.Fatal(err)
	}
	if err := s.RemoveHost("missing"); err == nil {
		t.Fatal("expected unknown host error")
	}
}
func TestStoreRejectsUnknownAndSymlink(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "config.json")
	if err := os.WriteFile(p, []byte(`{"version":1,"hosts":[],"typo":true}`), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := (Store{Path: p}).Load(); err == nil {
		t.Fatal("expected unknown field error")
	}
	target := filepath.Join(dir, "real")
	_ = os.WriteFile(target, []byte(`{"version":1,"hosts":[]}`), 0600)
	link := filepath.Join(dir, "link")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	if _, err := (Store{Path: link}).Load(); err == nil {
		t.Fatal("expected symlink rejection")
	}
}
func TestValidation(t *testing.T) {
	bad := []Host{{Name: "../x", Destination: "x"}, {Name: "ok", Destination: "-bad"}, {Name: "ok", Destination: "has space"}, {Name: "ok", Destination: "x", Sources: SourceOverrides{Pi: []string{"bad\npath"}}}, {Name: "ok", Destination: "x", Sources: SourceOverrides{Pi: []string{"relative"}}}}
	for _, h := range bad {
		if ValidateHost(h) == nil {
			t.Errorf("accepted %#v", h)
		}
	}
}
