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

	"github.com/mattsp1290/slurp/internal/safepath"
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
	return safepath.EnsureDir(path, 0700)
}

func (s Store) Load() (Config, error) {
	if err := safepath.CheckTrustedParents(s.Path); err != nil {
		return Config{}, err
	}
	if err := safepath.CheckNoSymlinkComponents(filepath.Dir(s.Path)); err != nil {
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
	lock, err := safepath.OpenLock(s.Path + ".lock")
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
	return safepath.SyncDir(dir)
}
