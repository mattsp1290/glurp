package safepath

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCheckTrustedParents(t *testing.T) {
	root := t.TempDir()
	if err := os.Chmod(root, 0700); err != nil {
		t.Fatal(err)
	}
	unsafe := filepath.Join(root, "unsafe")
	if err := os.Mkdir(unsafe, 0777); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(unsafe, 0777); err != nil {
		t.Fatal(err)
	}
	if err := CheckTrustedParents(filepath.Join(unsafe, "target")); err == nil {
		t.Fatal("accepted writable non-sticky parent")
	}
	if err := os.Chmod(unsafe, os.ModeSticky|0777); err != nil {
		t.Fatal(err)
	}
	if err := CheckTrustedParents(filepath.Join(unsafe, "target")); err != nil {
		t.Fatalf("rejected sticky parent: %v", err)
	}
}
