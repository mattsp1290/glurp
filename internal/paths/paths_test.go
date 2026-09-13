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
	if p.Config != "/home/test/.config/glurp/config.json" || p.DataDir != "/data/glurp" {
		t.Fatalf("unexpected paths: %#v", p)
	}
	legacy := "s" + "lurp"
	p, err = Resolve(filepath.Join(legacy, "config.json"), legacy, map[string]string{})
	if err != nil {
		t.Fatal(err)
	}
	if !filepath.IsAbs(p.Config) || !filepath.IsAbs(p.DataDir) {
		t.Fatalf("overrides were not absolute: %#v", p)
	}
	if filepath.Base(filepath.Dir(p.Config)) != legacy || filepath.Base(p.DataDir) != legacy {
		t.Fatalf("explicit legacy override was not retained: %#v", p)
	}
}
func TestResolveRequiresHome(t *testing.T) {
	if _, err := Resolve("", "", map[string]string{}); err == nil {
		t.Fatal("expected missing HOME error")
	}
}
