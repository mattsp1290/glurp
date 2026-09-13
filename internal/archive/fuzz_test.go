package archive

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/mattsp1290/glurp/internal/protocol"
)

func FuzzRelativePath(f *testing.F) {
	for _, s := range []string{"a.jsonl", "nested/a.jsonl", "../escape", "/absolute", "a/../../b", ""} {
		f.Add(s)
	}
	f.Fuzz(func(t *testing.T, p string) {
		if protocol.ValidateRelativePath(p) != nil {
			return
		}
		root := filepath.Join(string(os.PathSeparator), "archive", "harness")
		target := filepath.Join(root, filepath.FromSlash(p))
		rel, err := filepath.Rel(root, target)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
			t.Fatalf("accepted path escaped: %q", p)
		}
	})
}
