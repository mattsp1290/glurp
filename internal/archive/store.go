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
	"strings"

	"github.com/mattsp1290/slurp/internal/protocol"
	"github.com/mattsp1290/slurp/internal/safepath"
	"golang.org/x/sys/unix"
)

func (b *Batch) SetCollectorVersion(v string) {
	if v == "" {
		v = "unknown"
	}
	b.collectorVersion = v
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
	il, err := safepath.OpenLock(filepath.Join(ld, "identity.lock"))
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
	lf, err := safepath.OpenLock(filepath.Join(ld, harness+".lock"))
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
	if err = s.recoverTransaction(host, harness); err != nil {
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
	p := filepath.Join(b.store.Root, "state", b.host, b.harness+".json")
	if st, e := os.Lstat(p); e == nil && (st.Mode()&os.ModeSymlink != 0 || !st.Mode().IsRegular()) {
		return fmt.Errorf("unsafe prior manifest")
	}
	f, err := safepath.OpenRegular(p)
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
	f, e := safepath.OpenRegular(path)
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
