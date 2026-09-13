package config

import (
	"fmt"
	"strings"
	"unicode"
	"unicode/utf8"
)

const Version = 1

type Config struct {
	Version int    `json:"version"`
	Hosts   []Host `json:"hosts"`
}

type Host struct {
	Name        string          `json:"name"`
	Destination string          `json:"destination"`
	Sources     SourceOverrides `json:"sources"`
}

type SourceOverrides struct {
	Claude []string `json:"claude"`
	Codex  []string `json:"codex"`
	Pi     []string `json:"pi"`
}

func Empty() Config { return Config{Version: Version, Hosts: []Host{}} }

func Validate(c Config) error {
	if c.Version != Version {
		return fmt.Errorf("unsupported config version %d", c.Version)
	}
	seen := map[string]bool{}
	for i := range c.Hosts {
		h := &c.Hosts[i]
		if err := ValidateHost(*h); err != nil {
			return fmt.Errorf("host %q: %w", h.Name, err)
		}
		if seen[h.Name] {
			return fmt.Errorf("duplicate host name %q", h.Name)
		}
		seen[h.Name] = true
	}
	return nil
}

func ValidateHost(h Host) error {
	if len(h.Name) == 0 || len(h.Name) > 64 {
		return fmt.Errorf("name must contain 1 to 64 bytes")
	}
	for i, r := range h.Name {
		ok := r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || i > 0 && (r == '.' || r == '_' || r == '-')
		if !ok {
			return fmt.Errorf("invalid name %q (use lowercase letters, digits, '.', '_' or '-')", h.Name)
		}
	}
	if len(h.Destination) == 0 || len(h.Destination) > 512 || strings.HasPrefix(h.Destination, "-") {
		return fmt.Errorf("invalid SSH destination")
	}
	for _, r := range h.Destination {
		if unicode.IsSpace(r) || unicode.IsControl(r) {
			return fmt.Errorf("SSH destination must not contain whitespace or control characters")
		}
	}
	if !utf8.ValidString(h.Destination) {
		return fmt.Errorf("SSH destination is not valid UTF-8")
	}
	for _, roots := range [][]string{h.Sources.Claude, h.Sources.Codex, h.Sources.Pi} {
		for _, root := range roots {
			if root == "" {
				return fmt.Errorf("source path must not be empty")
			}
			if len(root) > 16<<10 {
				return fmt.Errorf("source path exceeds 16384 bytes")
			}
			if !strings.HasPrefix(root, "/") && !strings.HasPrefix(root, "~/") {
				return fmt.Errorf("source path must be absolute or begin with ~/")
			}
			if !utf8.ValidString(root) {
				return fmt.Errorf("source path is not valid UTF-8")
			}
			for _, r := range root {
				if unicode.IsControl(r) {
					return fmt.Errorf("source path contains a control character")
				}
			}
		}
	}
	return nil
}
