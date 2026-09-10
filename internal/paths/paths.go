package paths

import (
	"fmt"
	"path/filepath"
)

type Paths struct {
	Config  string
	DataDir string
}

func Resolve(configOverride, dataOverride string, env map[string]string) (Paths, error) {
	home := env["HOME"]
	base := func(key, fallback string) (string, error) {
		if v := env[key]; filepath.IsAbs(v) {
			return filepath.Clean(v), nil
		}
		if !filepath.IsAbs(home) {
			return "", fmt.Errorf("cannot resolve paths: HOME is not an absolute path")
		}
		return filepath.Join(home, fallback), nil
	}
	abs := func(v string) (string, error) {
		if v == "" {
			return "", nil
		}
		p, err := filepath.Abs(v)
		if err != nil {
			return "", err
		}
		return filepath.Clean(p), nil
	}
	cfg, err := abs(configOverride)
	if err != nil {
		return Paths{}, fmt.Errorf("resolve config path: %w", err)
	}
	if cfg == "" {
		b, e := base("XDG_CONFIG_HOME", ".config")
		if e != nil {
			return Paths{}, e
		}
		cfg = filepath.Join(b, "slurp", "config.json")
	}
	data, err := abs(dataOverride)
	if err != nil {
		return Paths{}, fmt.Errorf("resolve data path: %w", err)
	}
	if data == "" {
		b, e := base("XDG_DATA_HOME", filepath.Join(".local", "share"))
		if e != nil {
			return Paths{}, e
		}
		data = filepath.Join(b, "slurp")
	}
	return Paths{Config: cfg, DataDir: data}, nil
}
