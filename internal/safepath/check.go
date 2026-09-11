// Package safepath validates the trust boundary around path-based operations.
package safepath

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// CheckTrustedParents rejects parent chains that another local account can
// rewrite. Sticky world-writable directories such as /tmp are safe because
// the kernel restricts removal and replacement to the entry owner.
func CheckTrustedParents(target string) error {
	abs, err := filepath.Abs(target)
	if err != nil {
		return err
	}
	parent := filepath.Dir(abs)
	cur := string(os.PathSeparator)
	for _, part := range strings.Split(strings.TrimPrefix(parent, string(os.PathSeparator)), string(os.PathSeparator)) {
		if part == "" {
			continue
		}
		cur = filepath.Join(cur, part)
		st, e := os.Lstat(cur)
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return e
		}
		if st.Mode()&os.ModeSymlink != 0 || !st.IsDir() {
			return fmt.Errorf("unsafe path component %s", cur)
		}
		if st.Mode().Perm()&0022 != 0 && st.Mode()&os.ModeSticky == 0 {
			return fmt.Errorf("path component %s is writable by another account", cur)
		}
	}
	return nil
}

func SyncDir(path string) error {
	d, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open directory for sync: %w", err)
	}
	if err = d.Sync(); err != nil {
		_ = d.Close()
		return fmt.Errorf("sync directory: %w", err)
	}
	if err = d.Close(); err != nil {
		return fmt.Errorf("close directory after sync: %w", err)
	}
	return nil
}
