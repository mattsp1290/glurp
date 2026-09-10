package config

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/sys/unix"
)

type Store struct{ Path string }

func (s Store) AddHost(h Host) error {
	normalizeHost(&h)
	return s.mutate(func(c *Config) error {
		for _, x := range c.Hosts {
			if x.Name == h.Name {
				return fmt.Errorf("host %q already exists; remove it before re-adding", h.Name)
			}
		}
		c.Hosts = append(c.Hosts, h)
		return nil
	})
}
func (s Store) RemoveHost(name string) error {
	return s.mutate(func(c *Config) error {
		for i, x := range c.Hosts {
			if x.Name == name {
				c.Hosts = append(c.Hosts[:i], c.Hosts[i+1:]...)
				return nil
			}
		}
		return fmt.Errorf("unknown host %q", name)
	})
}

func ensureDir(path string) error {
	path = filepath.Clean(path)
	if err := rejectSymlinkComponents(path); err != nil {
		return err
	}
	st, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		if err := os.MkdirAll(path, 0700); err != nil {
			return err
		}
		return os.Chmod(path, 0700)
	}
	if err != nil {
		return err
	}
	if st.Mode()&os.ModeSymlink != 0 || !st.IsDir() {
		return fmt.Errorf("unsafe directory %s", path)
	}
	if st.Mode().Perm() != 0700 {
		if err := os.Chmod(path, 0700); err != nil {
			return err
		}
	}
	return nil
}

func rejectSymlinkComponents(path string) error {
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
		st, e := os.Lstat(cur)
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return e
		}
		if st.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("unsafe symlinked path component %s", cur)
		}
	}
	return nil
}

func openLock(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_CREAT|unix.O_RDWR|unix.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	f := os.NewFile(uintptr(fd), path)
	if err := f.Chmod(0600); err != nil {
		f.Close()
		return nil, err
	}
	return f, nil
}

func (s Store) Load() (Config, error) {
	if err := rejectSymlinkComponents(filepath.Dir(s.Path)); err != nil {
		return Config{}, err
	}
	if err := rejectSymlinkComponents(filepath.Dir(s.Path)); err != nil {
		return Config{}, err
	}
	lst, lerr := os.Lstat(s.Path)
	if errors.Is(lerr, os.ErrNotExist) {
		return Empty(), nil
	}
	if lerr != nil {
		return Config{}, fmt.Errorf("inspect config: %w", lerr)
	}
	if lst.Mode()&os.ModeSymlink != 0 || !lst.Mode().IsRegular() {
		return Config{}, fmt.Errorf("config is not a regular file")
	}
	fd, err := unix.Open(s.Path, unix.O_RDONLY|unix.O_NOFOLLOW, 0)
	if err != nil {
		return Config{}, fmt.Errorf("open config: %w", err)
	}
	f := os.NewFile(uintptr(fd), s.Path)
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return Config{}, err
	}
	if !st.Mode().IsRegular() {
		return Config{}, fmt.Errorf("config is not a regular file")
	}
	if st.Mode().Perm() != 0600 {
		if err := f.Chmod(0600); err != nil {
			return Config{}, fmt.Errorf("secure config: %w", err)
		}
	}
	var c Config
	d := json.NewDecoder(io.LimitReader(f, 16<<20))
	d.DisallowUnknownFields()
	if err := d.Decode(&c); err != nil {
		return Config{}, fmt.Errorf("invalid config: %w", err)
	}
	if err := d.Decode(&struct{}{}); err != io.EOF {
		return Config{}, fmt.Errorf("invalid config: trailing JSON")
	}
	if c.Hosts == nil {
		c.Hosts = []Host{}
	}
	for i := range c.Hosts {
		normalizeHost(&c.Hosts[i])
	}
	if err := Validate(c); err != nil {
		return Config{}, fmt.Errorf("invalid config: %w", err)
	}
	sort.Slice(c.Hosts, func(i, j int) bool { return c.Hosts[i].Name < c.Hosts[j].Name })
	return c, nil
}

func normalizeHost(h *Host) {
	if h.Sources.Claude == nil {
		h.Sources.Claude = []string{}
	}
	if h.Sources.Codex == nil {
		h.Sources.Codex = []string{}
	}
	if h.Sources.Pi == nil {
		h.Sources.Pi = []string{}
	}
}

func (s Store) mutate(fn func(*Config) error) error {
	dir := filepath.Dir(s.Path)
	if err := ensureDir(dir); err != nil {
		return fmt.Errorf("prepare config directory: %w", err)
	}
	lock, err := openLock(s.Path + ".lock")
	if err != nil {
		return err
	}
	defer lock.Close()
	if err := unix.Flock(int(lock.Fd()), unix.LOCK_EX); err != nil {
		return err
	}
	defer unix.Flock(int(lock.Fd()), unix.LOCK_UN)
	c, err := s.Load()
	if err != nil {
		return err
	}
	if err := fn(&c); err != nil {
		return err
	}
	sort.Slice(c.Hosts, func(i, j int) bool { return c.Hosts[i].Name < c.Hosts[j].Name })
	if err := Validate(c); err != nil {
		return err
	}
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	if err := enc.Encode(c); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".config-*.tmp")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)
	if err := tmp.Chmod(0600); err != nil {
		tmp.Close()
		return err
	}
	if _, err := tmp.Write(b.Bytes()); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmpName, s.Path); err != nil {
		return err
	}
	if d, err := os.Open(dir); err == nil {
		_ = d.Sync()
		_ = d.Close()
	}
	return nil
}
