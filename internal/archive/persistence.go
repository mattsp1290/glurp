package archive

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/mattsp1290/glurp/internal/safepath"
)

func secureDir(path string) error {
	return safepath.EnsureDir(path, 0700)
}
func secureUnder(root, target string) error {
	rel, err := filepath.Rel(root, target)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
		return fmt.Errorf("archive path escapes root")
	}
	if err := secureDir(root); err != nil {
		return err
	}
	cur := root
	if rel == "." {
		return nil
	}
	for _, part := range strings.Split(rel, string(os.PathSeparator)) {
		cur = filepath.Join(cur, part)
		if err := secureDir(cur); err != nil {
			return err
		}
	}
	return nil
}
func atomicJSON(path string, value any) error {
	if err := secureDir(filepath.Dir(path)); err != nil {
		return err
	}
	if st, e := os.Lstat(path); e == nil && (st.Mode()&os.ModeSymlink != 0 || !st.Mode().IsRegular()) {
		return fmt.Errorf("unsafe state target %s", path)
	}
	b, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	b = append(b, '\n')
	f, err := os.CreateTemp(filepath.Dir(path), ".write-*.tmp")
	if err != nil {
		return err
	}
	n := f.Name()
	defer os.Remove(n)
	if err = f.Chmod(0600); err == nil {
		_, err = f.Write(b)
	}
	if err == nil {
		err = f.Sync()
	}
	if e := f.Close(); err == nil {
		err = e
	}
	if err != nil {
		return err
	}
	if err = os.Rename(n, path); err != nil {
		return err
	}
	return safepath.SyncDir(filepath.Dir(path))
}
func (s Store) ensureIdentity(host, dest string) error {
	p := filepath.Join(s.Root, "state", host, "identity.json")
	if err := secureDir(filepath.Dir(p)); err != nil {
		return err
	}
	if st, e := os.Lstat(p); e == nil && (st.Mode()&os.ModeSymlink != 0 || !st.Mode().IsRegular()) {
		return fmt.Errorf("unsafe archive identity for host %q", host)
	}
	f, err := safepath.OpenRegular(p)
	if errors.Is(err, os.ErrNotExist) {
		return atomicJSON(p, Identity{Version: 1, Destination: dest})
	}
	if err != nil {
		return err
	}
	b, err := io.ReadAll(io.LimitReader(f, 1<<20))
	_ = f.Close()
	if err != nil {
		return err
	}
	var id Identity
	if json.Unmarshal(b, &id) != nil || id.Version != 1 {
		return fmt.Errorf("invalid archive identity for host %q", host)
	}
	if id.Destination != dest {
		return fmt.Errorf("archive host %q is bound to a different SSH destination; use a new host name", host)
	}
	return nil
}
