package archive

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/mattsp1290/slurp/internal/protocol"
	"golang.org/x/sys/unix"
)

type Entry struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
	Bytes  uint64 `json:"bytes"`
}
type Manifest struct {
	Version          int       `json:"version"`
	Destination      string    `json:"destination"`
	Harness          string    `json:"harness"`
	CollectorVersion string    `json:"collector_version"`
	CollectedAt      time.Time `json:"collected_at"`
	Roots            []string  `json:"roots,omitempty"`
	Entries          []Entry   `json:"entries"`
}
type Identity struct {
	Version     int    `json:"version"`
	Destination string `json:"destination"`
}
type Store struct{ Root string }
type staged struct {
	entry       Entry
	path, final string
	unchanged   bool
}
type Batch struct {
	store                      Store
	host, harness, destination string
	lock                       *os.File
	stageDir                   string
	files                      []staged
	seen                       map[string]bool
	roots                      []string
	collectorVersion           string
}

func (b *Batch) SetCollectorVersion(v string) {
	if v == "" {
		v = "unknown"
	}
	b.collectorVersion = v
}

func secureDir(path string) error {
	if err := rejectSymlinkComponents(path); err != nil {
		return err
	}
	st, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		missing := []string{}
		cur := path
		for {
			if _, e := os.Lstat(cur); e == nil {
				break
			} else if !errors.Is(e, os.ErrNotExist) {
				return e
			}
			missing = append(missing, cur)
			next := filepath.Dir(cur)
			if next == cur {
				return fmt.Errorf("cannot find archive path ancestor")
			}
			cur = next
		}
		for i := len(missing) - 1; i >= 0; i-- {
			if err := os.Mkdir(missing[i], 0700); err != nil && !errors.Is(err, os.ErrExist) {
				return err
			}
			if err := os.Chmod(missing[i], 0700); err != nil {
				return err
			}
		}
		return nil
	}
	if err != nil {
		return err
	}
	if st.Mode()&os.ModeSymlink != 0 || !st.IsDir() {
		return fmt.Errorf("unsafe archive directory %s", path)
	}
	if st.Mode().Perm() != 0700 {
		return os.Chmod(path, 0700)
	}
	return nil
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
			return fmt.Errorf("unsafe symlinked archive path component %s", cur)
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
func openRegularNoFollow(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_NOFOLLOW, 0)
	if err != nil {
		return nil, err
	}
	f := os.NewFile(uintptr(fd), path)
	st, err := f.Stat()
	if err != nil || !st.Mode().IsRegular() {
		f.Close()
		return nil, fmt.Errorf("archive target is not a regular file")
	}
	return f, nil
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
	d, e := os.Open(filepath.Dir(path))
	if e == nil {
		_ = d.Sync()
		_ = d.Close()
	}
	return nil
}
func (s Store) ensureIdentity(host, dest string) error {
	p := filepath.Join(s.Root, "state", host, "identity.json")
	if err := secureDir(filepath.Dir(p)); err != nil {
		return err
	}
	if st, e := os.Lstat(p); e == nil && (st.Mode()&os.ModeSymlink != 0 || !st.Mode().IsRegular()) {
		return fmt.Errorf("unsafe archive identity for host %q", host)
	}
	f, err := openRegularNoFollow(p)
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
func (s Store) BeginHostHarness(host, destination, harness string) (*Batch, error) {
	if err := protocol.ValidateRelativePath(host); err != nil || strings.Contains(host, "/") {
		return nil, fmt.Errorf("invalid archive host name")
	}
	if err := protocol.ValidateRelativePath(harness); err != nil || strings.Contains(harness, "/") {
		return nil, fmt.Errorf("invalid harness name")
	}
	if err := secureDir(s.Root); err != nil {
		return nil, err
	}
	ld := filepath.Join(s.Root, "locks", host)
	if err := secureUnder(s.Root, ld); err != nil {
		return nil, err
	}
	il, err := openLock(filepath.Join(ld, "identity.lock"))
	if err != nil {
		return nil, err
	}
	if err = unix.Flock(int(il.Fd()), unix.LOCK_EX); err != nil {
		il.Close()
		return nil, err
	}
	err = s.ensureIdentity(host, destination)
	_ = unix.Flock(int(il.Fd()), unix.LOCK_UN)
	_ = il.Close()
	if err != nil {
		return nil, err
	}
	lf, err := openLock(filepath.Join(ld, harness+".lock"))
	if err != nil {
		return nil, err
	}
	if err = unix.Flock(int(lf.Fd()), unix.LOCK_EX|unix.LOCK_NB); err != nil {
		lf.Close()
		return nil, fmt.Errorf("collection already active for %s/%s", host, harness)
	}
	td := filepath.Join(s.Root, ".tmp", host, harness)
	if err = secureUnder(s.Root, td); err != nil {
		lf.Close()
		return nil, err
	}
	if stale, e := os.ReadDir(td); e == nil {
		for _, x := range stale {
			if x.IsDir() && strings.HasPrefix(x.Name(), "run-") {
				_ = os.RemoveAll(filepath.Join(td, x.Name()))
			}
		}
	}
	run, err := os.MkdirTemp(td, "run-")
	if err != nil {
		lf.Close()
		return nil, err
	}
	_ = os.Chmod(run, 0700)
	return &Batch{store: s, host: host, harness: harness, destination: destination, lock: lf, stageDir: run, seen: map[string]bool{}}, nil
}
func (b *Batch) BindRoots(roots []string) error {
	b.roots = append([]string(nil), roots...)
	if len(roots) == 0 {
		return nil
	}
	p := filepath.Join(b.store.Root, "state", b.host, b.harness+".json")
	if st, e := os.Lstat(p); e == nil && (st.Mode()&os.ModeSymlink != 0 || !st.Mode().IsRegular()) {
		return fmt.Errorf("unsafe prior manifest")
	}
	f, err := openRegularNoFollow(p)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	data, err := io.ReadAll(io.LimitReader(f, 16<<20))
	_ = f.Close()
	if err != nil {
		return err
	}
	var prior Manifest
	if err = json.Unmarshal(data, &prior); err != nil {
		return fmt.Errorf("invalid prior manifest: %w", err)
	}
	if len(prior.Roots) > 0 && (len(roots) < len(prior.Roots) || !slices.Equal(prior.Roots, roots[:len(prior.Roots)])) {
		return fmt.Errorf("explicit source root order changed; use the original order or a new host name")
	}
	return nil
}
func hashFile(path string) (string, uint64, error) {
	lst, e := os.Lstat(path)
	if e != nil {
		return "", 0, e
	}
	if lst.Mode()&os.ModeSymlink != 0 || !lst.Mode().IsRegular() {
		return "", 0, fmt.Errorf("archive target is not a regular file")
	}
	f, e := openRegularNoFollow(path)
	if e != nil {
		return "", 0, e
	}
	defer f.Close()
	st, e := f.Stat()
	if e != nil || !st.Mode().IsRegular() {
		return "", 0, fmt.Errorf("archive target is not regular")
	}
	h := sha256.New()
	n, e := io.Copy(h, f)
	return hex.EncodeToString(h.Sum(nil)), uint64(n), e
}
func (b *Batch) Put(path string, size uint64, body io.Reader) (bool, error) {
	if err := protocol.ValidateRelativePath(path); err != nil {
		return false, err
	}
	if b.seen[path] {
		return false, fmt.Errorf("duplicate path %q", path)
	}
	b.seen[path] = true
	final := filepath.Join(b.store.Root, "hosts", b.host, b.harness, filepath.FromSlash(path))
	root := filepath.Join(b.store.Root, "hosts", b.host, b.harness)
	rel, err := filepath.Rel(root, final)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
		return false, fmt.Errorf("archive path escapes root")
	}
	tmp := filepath.Join(b.stageDir, fmt.Sprintf("%06d", len(b.files)))
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return false, err
	}
	h := sha256.New()
	n, err := io.CopyN(io.MultiWriter(f, h), body, int64(size))
	if err == nil && uint64(n) != size {
		err = io.ErrUnexpectedEOF
	}
	if err == nil {
		err = f.Sync()
	}
	if e := f.Close(); err == nil {
		err = e
	}
	if err != nil {
		return false, err
	}
	e := Entry{Path: path, SHA256: hex.EncodeToString(h.Sum(nil)), Bytes: size}
	unchanged := false
	if got, gotSize, x := hashFile(final); x == nil && got == e.SHA256 && gotSize == size {
		unchanged = true
		if err := os.Chmod(final, 0600); err != nil {
			return false, err
		}
		_ = os.Remove(tmp)
	}
	b.files = append(b.files, staged{entry: e, path: tmp, final: final, unchanged: unchanged})
	return unchanged, nil
}
func (b *Batch) PutStream(path string, max uint64, body io.Reader) (bool, uint64, error) {
	if max == ^uint64(0) {
		return false, 0, fmt.Errorf("stream limit too large")
	}
	tmp, err := os.CreateTemp(b.stageDir, "stream-")
	if err != nil {
		return false, 0, err
	}
	name := tmp.Name()
	h := sha256.New()
	n, err := io.Copy(io.MultiWriter(tmp, h), io.LimitReader(body, int64(max)+1))
	if err == nil {
		err = tmp.Sync()
	}
	if e := tmp.Close(); err == nil {
		err = e
	}
	if err != nil {
		_ = os.Remove(name)
		return false, 0, err
	}
	if uint64(n) > max {
		_ = os.Remove(name)
		return false, uint64(n), fmt.Errorf("artifact %q exceeds file size limit", path)
	}
	f, err := os.Open(name)
	if err != nil {
		return false, 0, err
	}
	defer f.Close()
	unchanged, err := b.Put(path, uint64(n), f)
	_ = os.Remove(name)
	return unchanged, uint64(n), err
}
func (b *Batch) PutJSONStream(path string, max uint64, body io.Reader) (bool, uint64, error) {
	if max > uint64(^uint64(0)>>1)-1 {
		return false, 0, fmt.Errorf("stream limit too large")
	}
	tmp, err := os.CreateTemp(b.stageDir, "json-")
	if err != nil {
		return false, 0, err
	}
	name := tmp.Name()
	defer os.Remove(name)
	n, err := io.Copy(tmp, io.LimitReader(body, int64(max)+1))
	if err == nil {
		err = tmp.Sync()
	}
	if e := tmp.Close(); err == nil {
		err = e
	}
	if err != nil {
		return false, uint64(n), err
	}
	if uint64(n) > max {
		return false, uint64(n), fmt.Errorf("artifact %q exceeds file size limit", path)
	}
	f, err := os.Open(name)
	if err != nil {
		return false, 0, err
	}
	dec := json.NewDecoder(f)
	var value any
	if err = dec.Decode(&value); err == nil {
		var extra any
		if e := dec.Decode(&extra); e != io.EOF {
			if e == nil {
				err = fmt.Errorf("multiple JSON documents")
			} else {
				err = e
			}
		}
	}
	_ = f.Close()
	if err != nil {
		return false, uint64(n), fmt.Errorf("artifact %q is not one JSON document", path)
	}
	f, err = os.Open(name)
	if err != nil {
		return false, 0, err
	}
	defer f.Close()
	u, err := b.Put(path, uint64(n), f)
	return u, uint64(n), err
}
func (b *Batch) Commit(now time.Time) (written, unchanged uint64, err error) {
	defer b.close()
	for _, s := range b.files {
		if s.unchanged {
			unchanged++
			continue
		}
		if err = secureUnder(b.store.Root, filepath.Dir(s.final)); err != nil {
			return
		}
		if st, e := os.Lstat(s.final); e == nil && (!st.Mode().IsRegular() || st.Mode()&os.ModeSymlink != 0) {
			err = fmt.Errorf("unsafe archive target %s", s.entry.Path)
			return
		}
		if err = os.Rename(s.path, s.final); err != nil {
			return
		}
		if d, e := os.Open(filepath.Dir(s.final)); e == nil {
			if e = d.Sync(); e != nil {
				_ = d.Close()
				err = e
				return
			}
			_ = d.Close()
		} else {
			err = e
			return
		}
		written++
	}
	entries := make([]Entry, 0, len(b.files))
	for _, s := range b.files {
		entries = append(entries, s.entry)
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Path < entries[j].Path })
	err = atomicJSON(filepath.Join(b.store.Root, "state", b.host, b.harness+".json"), Manifest{Version: 1, Destination: b.destination, Harness: b.harness, CollectorVersion: b.collectorVersion, CollectedAt: now.UTC(), Roots: b.roots, Entries: entries})
	return
}
func (b *Batch) Abort() { b.close() }
func (b *Batch) close() {
	if b.lock == nil {
		return
	}
	_ = os.RemoveAll(b.stageDir)
	_ = unix.Flock(int(b.lock.Fd()), unix.LOCK_UN)
	_ = b.lock.Close()
	b.lock = nil
}
