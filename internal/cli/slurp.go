package cli

import (
	"fmt"
	"sort"
	"time"

	"github.com/mattsp1290/slurp/internal/archive"
	"github.com/mattsp1290/slurp/internal/collect"
	"github.com/mattsp1290/slurp/internal/harness"
	"github.com/spf13/cobra"
)

func newSlurpCommand(d Dependencies, o *options) *cobra.Command {
	var hs []string
	var jobs int
	var timeout time.Duration
	var maxFile, maxFiles, maxTotal uint64
	cmd := &cobra.Command{Use: "slurp [host-name ...]", Short: "Collect chat archives from configured hosts", Args: cobra.ArbitraryArgs, RunE: func(cmd *cobra.Command, args []string) error {
		if jobs < 1 || jobs > 32 {
			return fmt.Errorf("--jobs must be between 1 and 32")
		}
		if timeout < 0 {
			return fmt.Errorf("--operation-timeout must not be negative")
		}
		if maxFile == 0 {
			return fmt.Errorf("--max-file-bytes must be greater than zero")
		}
		if maxFiles == 0 {
			return fmt.Errorf("--max-files must be greater than zero")
		}
		if maxFiles > 1000000 {
			return fmt.Errorf("--max-files must not exceed 1000000")
		}
		ids, e := harness.Parse(hs)
		if e != nil {
			return e
		}
		store, p, e := resolveStore(d, o)
		if e != nil {
			return e
		}
		cfg, e := store.Load()
		if e != nil {
			return e
		}
		if len(cfg.Hosts) == 0 {
			return fmt.Errorf("no hosts configured; run 'slurp host add <name> <destination>'")
		}
		byName := map[string]struct{}{}
		for _, n := range args {
			if _, ok := byName[n]; ok {
				return fmt.Errorf("host %q selected more than once", n)
			}
			byName[n] = struct{}{}
		}
		selected := cfg.Hosts
		if len(args) > 0 {
			selected = nil
			known := map[string]bool{}
			for _, h := range cfg.Hosts {
				known[h.Name] = true
			}
			for _, n := range args {
				if !known[n] {
					return fmt.Errorf("unknown host %q", n)
				}
				for _, h := range cfg.Hosts {
					if h.Name == n {
						selected = append(selected, h)
						break
					}
				}
			}
		}
		sort.Slice(selected, func(i, j int) bool { return selected[i].Name < selected[j].Name })
		if d.NewRunner == nil {
			return fmt.Errorf("SSH runner is not configured")
		}
		runner, e := d.NewRunner()
		if e != nil {
			return e
		}
		co := collect.Coordinator{Runner: runner, Archive: archive.Store{Root: p.DataDir}, Now: d.Now}
		report := co.Run(cmd.Context(), collect.Selection{Hosts: selected, Harnesses: ids, Jobs: jobs, Timeout: timeout, Limits: harness.Limits{MaxFileBytes: maxFile, MaxFiles: maxFiles, MaxTotalBytes: maxTotal}})
		report.Print(d.Stdout)
		if report.Failed() {
			return fmt.Errorf("collection completed with failures")
		}
		return nil
	}}
	cmd.Flags().StringSliceVar(&hs, "harness", nil, "harness to collect (repeatable: claude, codex, opencode, pi)")
	cmd.Flags().IntVar(&jobs, "jobs", 4, "maximum concurrent host jobs")
	cmd.Flags().DurationVar(&timeout, "operation-timeout", 30*time.Minute, "timeout for each host/harness operation (0 disables)")
	cmd.Flags().Uint64Var(&maxFile, "max-file-bytes", 2147483648, "maximum bytes per artifact")
	cmd.Flags().Uint64Var(&maxFiles, "max-files", 1000000, "maximum artifacts per harness")
	cmd.Flags().Uint64Var(&maxTotal, "max-total-bytes", 107374182400, "maximum bytes per harness (0 disables)")
	return cmd
}
