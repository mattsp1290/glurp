// Package safepath validates the trust boundary around path-based operations.
package safepath

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"golang.org/x/sys/unix"
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

// CheckNoSymlinkComponents rejects an existing symlink anywhere in path.
func CheckNoSymlinkComponents(path string) error {
	abs, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	cur := string(os.PathSeparator)
	for _, part := range strings.Split(strings.TrimPrefix(abs, string(os.PathSeparator)), string(os.PathSeparator)) {
		if part == "" {
			continue
		}
		cur = filepath.Join(cur, part)
		st, err := os.Lstat(cur)
		if errors.Is(err, os.ErrNotExist) {
			continue
		}
		if err != nil {
			return err
		}
		if st.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("unsafe symlinked path component %s", cur)
		}
	}
	return nil
}

// EnsureDir creates a private directory chain or secures an existing leaf.
func EnsureDir(path string, mode os.FileMode) error {
	path = filepath.Clean(path)
	if err := CheckTrustedParents(path); err != nil {
		return err
	}
	if err := CheckNoSymlinkComponents(path); err != nil {
		return err
	}
	st, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		missing := []string{}
		cur := path
		for {
			if _, inspectErr := os.Lstat(cur); inspectErr == nil {
				break
			} else if !errors.Is(inspectErr, os.ErrNotExist) {
				return inspectErr
			}
			missing = append(missing, cur)
			next := filepath.Dir(cur)
			if next == cur {
				return fmt.Errorf("cannot find existing path ancestor")
			}
			cur = next
		}
		for i := len(missing) - 1; i >= 0; i-- {
			if err := os.Mkdir(missing[i], mode); err != nil && !errors.Is(err, os.ErrExist) {
				return err
			}
			if err := os.Chmod(missing[i], mode); err != nil {
				return err
			}
		}
		return nil
	}
	if err != nil {
		return err
	}
	if st.Mode()&os.ModeSymlink != 0 || !st.IsDir() {
		return fmt.Errorf("unsafe directory %s", path)
	}
	if st.Mode().Perm() != mode.Perm() {
		return os.Chmod(path, mode)
	}
	return nil
}

// OpenRegular opens a regular file without following a final symlink.
func OpenRegular(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_NOFOLLOW, 0)
	if err != nil {
		return nil, err
	}
	f := os.NewFile(uintptr(fd), path)
	st, err := f.Stat()
	if err != nil || !st.Mode().IsRegular() {
		_ = f.Close()
		return nil, fmt.Errorf("path is not a regular file")
	}
	return f, nil
}

// OpenLock opens a private lock file without following a final symlink.
func OpenLock(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_CREAT|unix.O_RDWR|unix.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	f := os.NewFile(uintptr(fd), path)
	if err := f.Chmod(0600); err != nil {
		_ = f.Close()
		return nil, err
	}
	return f, nil
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
