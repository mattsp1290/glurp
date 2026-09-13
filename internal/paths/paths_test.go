package paths

import (
	"path/filepath"
	"testing"
)

func TestResolve(t *testing.T) {
	p, err := Resolve("", "", map[string]string{"HOME": "/home/test", "XDG_CONFIG_HOME": "relative", "XDG_DATA_HOME": "/data"})
	if err != nil {
		t.Fatal(err)
	}
	if p.Config != "/home/test/.config/slurp/config.json" || p.DataDir != "/data/slurp" {
		t.Fatalf("unexpected paths: %#v", p)
	}
	p, err = Resolve("cfg.json", "archive", map[string]string{})
	if err != nil {
		t.Fatal(err)
	}
	if !filepath.IsAbs(p.Config) || !filepath.IsAbs(p.DataDir) {
		t.Fatalf("overrides were not absolute: %#v", p)
	}
}
func TestResolveRequiresHome(t *testing.T) {
	if _, err := Resolve("", "", map[string]string{}); err == nil {
		t.Fatal("expected missing HOME error")
	}
}
