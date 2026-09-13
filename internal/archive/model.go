package archive

import (
	"os"
	"time"
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
	TransactionID    string    `json:"transaction_id,omitempty"`
}

type Identity struct {
	Version     int    `json:"version"`
	Destination string `json:"destination"`
}

type transaction struct {
	Version int                `json:"version"`
	ID      string             `json:"id"`
	Stage   string             `json:"stage"`
	Entries []transactionEntry `json:"entries"`
}

type transactionEntry struct {
	Path     string `json:"path"`
	HadFinal bool   `json:"had_final"`
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
