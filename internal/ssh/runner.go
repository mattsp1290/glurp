package ssh

import (
	"context"
	"fmt"
	"io"
	"os/exec"
	"sync/atomic"
	"syscall"
	"time"
)

type Result struct {
	ExitCode    int
	StderrBytes int64
}
type Runner struct{ Path string }

func New() (Runner, error) {
	p, err := exec.LookPath("ssh")
	if err != nil {
		return Runner{}, fmt.Errorf("OpenSSH ssh executable not found")
	}
	return Runner{Path: p}, nil
}

type countWriter struct{ n atomic.Int64 }

func (w *countWriter) Write(p []byte) (int, error) { w.n.Add(int64(len(p))); return len(p), nil }

func (r Runner) Run(ctx context.Context, destination, script string, consume func(io.Reader) error) (Result, error) {
	cmd := exec.CommandContext(ctx, r.Path, "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", destination, "sh -s -- 1")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return nil
		}
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)
	}
	cmd.WaitDelay = 2 * time.Second
	cmd.Stdin = &stringReader{s: script}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return Result{}, err
	}
	cw := &countWriter{}
	cmd.Stderr = cw
	if err := cmd.Start(); err != nil {
		return Result{}, fmt.Errorf("start ssh: %w", err)
	}
	done := make(chan struct{})
	go func() {
		select {
		case <-ctx.Done():
			timer := time.NewTimer(2 * time.Second)
			defer timer.Stop()
			select {
			case <-timer.C:
				_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
			case <-done:
			}
		case <-done:
		}
	}()
	consumeErr := consume(stdout)
	var forceKill *time.Timer
	if consumeErr != nil {
		_ = stdout.Close()
		_ = cmd.Cancel()
		forceKill = time.AfterFunc(2*time.Second, func() { _ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) })
	}
	waitErr := cmd.Wait()
	close(done)
	if forceKill != nil {
		forceKill.Stop()
	}
	res := Result{StderrBytes: cw.n.Load()}
	if cmd.ProcessState != nil {
		res.ExitCode = cmd.ProcessState.ExitCode()
	}
	if consumeErr != nil {
		return res, consumeErr
	}
	if waitErr != nil {
		return res, fmt.Errorf("ssh exited with status %d (remote diagnostics suppressed: %t)", res.ExitCode, res.StderrBytes > 0)
	}
	return res, nil
}

type stringReader struct {
	s string
	i int
}

func (r *stringReader) Read(p []byte) (int, error) {
	if r.i >= len(r.s) {
		return 0, io.EOF
	}
	n := copy(p, r.s[r.i:])
	r.i += n
	return n, nil
}
