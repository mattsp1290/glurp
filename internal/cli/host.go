package cli

import (
	"encoding/json"
	"fmt"

	"github.com/mattsp1290/glurp/internal/config"
	pathutil "github.com/mattsp1290/glurp/internal/paths"
	"github.com/spf13/cobra"
)

func resolveStore(d Dependencies, o *options) (config.Store, pathutil.Paths, error) {
	p, e := pathutil.Resolve(o.configPath, o.dataDir, d.Environ)
	return config.Store{Path: p.Config}, p, e
}
func newHostCommand(d Dependencies, o *options) *cobra.Command {
	host := &cobra.Command{Use: "host", Short: "Manage SSH destinations"}
	var src config.SourceOverrides
	add := &cobra.Command{Use: "add <name> <destination>", Short: "Add an SSH destination", Args: cobra.ExactArgs(2), RunE: func(cmd *cobra.Command, args []string) error {
		s, p, e := resolveStore(d, o)
		if e != nil {
			return e
		}
		h := config.Host{Name: args[0], Destination: args[1], Sources: src}
		if e = config.ValidateHost(h); e != nil {
			return e
		}
		e = s.AddHost(h)
		if e == nil {
			fmt.Fprintf(d.Stdout, "added host %s in %s\n", h.Name, p.Config)
		}
		return e
	}}
	add.Flags().StringArrayVar(&src.Claude, "claude-path", nil, "required remote Claude transcript root (repeatable)")
	add.Flags().StringArrayVar(&src.Codex, "codex-path", nil, "required remote Codex root (repeatable)")
	add.Flags().StringArrayVar(&src.Pi, "pi-path", nil, "required remote pi session root (repeatable)")
	var asJSON bool
	list := &cobra.Command{Use: "list", Short: "List SSH destinations", Args: cobra.NoArgs, RunE: func(cmd *cobra.Command, args []string) error {
		s, _, e := resolveStore(d, o)
		if e != nil {
			return e
		}
		c, e := s.Load()
		if e != nil {
			return e
		}
		if asJSON {
			b, _ := json.Marshal(c.Hosts)
			fmt.Fprintln(d.Stdout, string(b))
			return nil
		}
		if len(c.Hosts) == 0 {
			fmt.Fprintln(d.Stdout, "no hosts configured; add one with 'glurp host add <name> <destination>'")
			return nil
		}
		for _, h := range c.Hosts {
			fmt.Fprintf(d.Stdout, "%s\t%s\n", h.Name, h.Destination)
		}
		return nil
	}}
	list.Flags().BoolVar(&asJSON, "json", false, "emit JSON")
	remove := &cobra.Command{Use: "remove <name>", Short: "Remove an SSH destination", Args: cobra.ExactArgs(1), RunE: func(cmd *cobra.Command, args []string) error {
		s, p, e := resolveStore(d, o)
		if e != nil {
			return e
		}
		e = s.RemoveHost(args[0])
		if e == nil {
			fmt.Fprintf(d.Stdout, "removed host %s from %s; archived data remains\n", args[0], p.Config)
		}
		return e
	}}
	host.AddCommand(add, list, remove)
	return host
}
