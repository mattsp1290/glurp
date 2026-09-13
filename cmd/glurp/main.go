package main

import (
	"context"
	"os"
	"syscall"

	"charm.land/fang/v2"
	"github.com/mattsp1290/glurp/internal/buildinfo"
	"github.com/mattsp1290/glurp/internal/cli"
)

func main() {
	root := cli.NewRoot(cli.DefaultDependencies())
	if err := fang.Execute(context.Background(), root, fang.WithVersion(buildinfo.Version), fang.WithCommit(buildinfo.Commit), fang.WithNotifySignal(os.Interrupt, syscall.SIGTERM)); err != nil {
		os.Exit(1)
	}
}
