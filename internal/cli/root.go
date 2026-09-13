package cli

import (
	"io"
	"os"
	"time"

	"github.com/mattsp1290/slurp/internal/harness"
	sshtransport "github.com/mattsp1290/slurp/internal/ssh"
	"github.com/spf13/cobra"
)

type Dependencies struct {
	Stdout, Stderr io.Writer
	Environ        map[string]string
	Now            func() time.Time
	NewRunner      func() (harness.Runner, error)
}
type options struct{ configPath, dataDir string }

func DefaultDependencies() Dependencies {
	env := map[string]string{}
	for _, kv := range os.Environ() {
		for i := 0; i < len(kv); i++ {
			if kv[i] == '=' {
				env[kv[:i]] = kv[i+1:]
				break
			}
		}
	}
	return Dependencies{Stdout: os.Stdout, Stderr: os.Stderr, Environ: env, Now: time.Now, NewRunner: func() (harness.Runner, error) { return sshtransport.New() }}
}

func NewRoot(d Dependencies) *cobra.Command {
	if d.Stdout == nil {
		d.Stdout = io.Discard
	}
	if d.Stderr == nil {
		d.Stderr = io.Discard
	}
	if d.Now == nil {
		d.Now = time.Now
	}
	if d.Environ == nil {
		d.Environ = map[string]string{}
	}
	o := &options{}
	cmd := &cobra.Command{Use: "slurp", Short: "Privately archive remote AI coding chats", Args: cobra.NoArgs, SilenceUsage: true, RunE: func(cmd *cobra.Command, args []string) error { return cmd.Help() }}
	cmd.SetOut(d.Stdout)
	cmd.SetErr(d.Stderr)
	cmd.PersistentFlags().StringVar(&o.configPath, "config", "", "configuration file path")
	cmd.PersistentFlags().StringVar(&o.dataDir, "data-dir", "", "archive data directory")
	cmd.AddCommand(newHostCommand(d, o), newSlurpCommand(d, o))
	return cmd
}
