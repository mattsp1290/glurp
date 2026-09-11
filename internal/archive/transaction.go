package archive

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/mattsp1290/slurp/internal/protocol"
	"github.com/mattsp1290/slurp/internal/safepath"
)

func (s Store) recoverTransaction(host, harness string) error {
	journalPath := filepath.Join(s.Root, "state", host, harness+".txn.json")
	f, err := safepath.OpenRegular(journalPath)
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
	var tx transaction
	if err = json.Unmarshal(data, &tx); err != nil {
		return fmt.Errorf("invalid archive transaction: %w", err)
	}
	if tx.Version != 1 || tx.ID != tx.Stage || !strings.HasPrefix(tx.Stage, "run-") || strings.ContainsAny(tx.Stage, "/\\") {
		return fmt.Errorf("invalid archive transaction metadata")
	}
	manifestPath := filepath.Join(s.Root, "state", host, harness+".json")
	committed := false
	if mf, e := safepath.OpenRegular(manifestPath); e == nil {
		mb, _ := io.ReadAll(io.LimitReader(mf, 16<<20))
		_ = mf.Close()
		var m Manifest
		if json.Unmarshal(mb, &m) == nil && m.TransactionID == tx.ID {
			committed = true
		}
	}
	stageDir := filepath.Join(s.Root, ".tmp", host, harness, tx.Stage)
	for i := len(tx.Entries) - 1; i >= 0; i-- {
		entry := tx.Entries[i]
		if err := protocol.ValidateRelativePath(entry.Path); err != nil {
			return fmt.Errorf("invalid archive transaction path")
		}
		final := filepath.Join(s.Root, "hosts", host, harness, filepath.FromSlash(entry.Path))
		backup := filepath.Join(stageDir, fmt.Sprintf("backup-%06d", i))
		if committed {
			continue
		}
		if entry.HadFinal {
			if st, e := os.Lstat(backup); e == nil {
				if !st.Mode().IsRegular() {
					return fmt.Errorf("invalid transaction backup")
				}
				if fst, fe := os.Lstat(final); fe == nil {
					if !fst.Mode().IsRegular() || fst.Mode()&os.ModeSymlink != 0 {
						return fmt.Errorf("unsafe transaction target")
					}
					if err := os.Remove(final); err != nil {
						return err
					}
				}
				if err := secureUnder(s.Root, filepath.Dir(final)); err != nil {
					return err
				}
				if err := os.Rename(backup, final); err != nil {
					return err
				}
				if err := safepath.SyncDir(filepath.Dir(final)); err != nil {
					return err
				}
			}
		} else if st, e := os.Lstat(final); e == nil {
			if !st.Mode().IsRegular() || st.Mode()&os.ModeSymlink != 0 {
				return fmt.Errorf("unsafe transaction target")
			}
			if err := os.Remove(final); err != nil {
				return err
			}
			if err := safepath.SyncDir(filepath.Dir(final)); err != nil {
				return err
			}
		}
	}
	if err := os.Remove(journalPath); err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	if err := safepath.SyncDir(filepath.Dir(journalPath)); err != nil {
		return err
	}
	if err := os.RemoveAll(stageDir); err != nil {
		return err
	}
	return nil
}

func (b *Batch) Commit(now time.Time) (written, unchanged uint64, err error) {
	entries := make([]Entry, 0, len(b.files))
	changes := make([]staged, 0, len(b.files))
	for _, s := range b.files {
		entries = append(entries, s.entry)
		if s.unchanged {
			unchanged++
		} else {
			changes = append(changes, s)
		}
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Path < entries[j].Path })
	manifestPath := filepath.Join(b.store.Root, "state", b.host, b.harness+".json")
	manifest := Manifest{Version: 1, Destination: b.destination, Harness: b.harness, CollectorVersion: b.collectorVersion, CollectedAt: now.UTC(), Roots: b.roots, Entries: entries}
	if len(changes) == 0 {
		defer b.close()
		err = atomicJSON(manifestPath, manifest)
		return
	}
	txID := filepath.Base(b.stageDir)
	tx := transaction{Version: 1, ID: txID, Stage: txID, Entries: make([]transactionEntry, 0, len(changes))}
	for _, s := range changes {
		if err = secureUnder(b.store.Root, filepath.Dir(s.final)); err != nil {
			b.close()
			return
		}
		had := false
		if st, e := os.Lstat(s.final); e == nil {
			if !st.Mode().IsRegular() || st.Mode()&os.ModeSymlink != 0 {
				err = fmt.Errorf("unsafe archive target %s", s.entry.Path)
				b.close()
				return
			}
			had = true
		} else if !errors.Is(e, os.ErrNotExist) {
			err = e
			b.close()
			return
		}
		tx.Entries = append(tx.Entries, transactionEntry{Path: s.entry.Path, HadFinal: had})
	}
	journalPath := filepath.Join(b.store.Root, "state", b.host, b.harness+".txn.json")
	if err = atomicJSON(journalPath, tx); err != nil {
		b.close()
		return
	}
	defer func() {
		if err != nil {
			if recoverErr := b.store.recoverTransaction(b.host, b.harness); recoverErr != nil {
				err = errors.Join(err, fmt.Errorf("recover archive transaction: %w", recoverErr))
			}
		}
		b.close()
	}()
	for i, s := range changes {
		if tx.Entries[i].HadFinal {
			backup := filepath.Join(b.stageDir, fmt.Sprintf("backup-%06d", i))
			if err = os.Rename(s.final, backup); err != nil {
				return
			}
			if err = safepath.SyncDir(b.stageDir); err != nil {
				return
			}
			if err = safepath.SyncDir(filepath.Dir(s.final)); err != nil {
				return
			}
		}
	}
	for _, s := range changes {
		if err = os.Rename(s.path, s.final); err != nil {
			return
		}
		if err = safepath.SyncDir(filepath.Dir(s.final)); err != nil {
			return
		}
		if err = safepath.SyncDir(b.stageDir); err != nil {
			return
		}
		written++
	}
	manifest.TransactionID = txID
	if err = atomicJSON(manifestPath, manifest); err != nil {
		return
	}
	if err = os.Remove(journalPath); err != nil {
		return
	}
	if err = safepath.SyncDir(filepath.Dir(journalPath)); err != nil {
		return
	}
	return
}
