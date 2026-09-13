package integration

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func run(t *testing.T, env []string, bin string, args ...string) (string, error) {
	t.Helper()
	cmd := exec.Command(bin, args...)
	cmd.Env = env
	b, e := cmd.CombinedOutput()
	return string(b), e
}
func copyFile(t *testing.T, src, dst string) {
	t.Helper()
	b, e := os.ReadFile(src)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.MkdirAll(filepath.Dir(dst), 0700); e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(dst, b, 0600); e != nil {
		t.Fatal(e)
	}
}
func TestCLIEndToEndWithFakeSSH(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX client only")
	}
	repo := filepath.Clean(filepath.Join("..", ".."))
	tmp := t.TempDir()
	if e := os.Chmod(tmp, 0700); e != nil {
		t.Fatal(e)
	}
	bin := filepath.Join(tmp, "glurp")
	cmd := exec.Command("go", "build", "-o", bin, "./cmd/glurp")
	cmd.Dir = repo
	if b, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("build: %v\n%s", e, b)
	}
	home := filepath.Join(tmp, "remote-home")
	copyFile(t, filepath.Join(repo, "testdata/remote/claude/projects/demo/session.jsonl"), filepath.Join(home, ".claude/projects/demo/session.jsonl"))
	copyFile(t, filepath.Join(repo, "testdata/remote/codex/sessions/2026/09/rollout.jsonl"), filepath.Join(home, ".codex/sessions/2026/09/rollout.jsonl"))
	copyFile(t, filepath.Join(repo, "testdata/remote/codex/archived_sessions/rollout.jsonl.zst"), filepath.Join(home, ".codex/archived_sessions/rollout.jsonl.zst"))
	copyFile(t, filepath.Join(repo, "testdata/remote/codex/session_index.jsonl"), filepath.Join(home, ".codex/session_index.jsonl"))
	copyFile(t, filepath.Join(repo, "testdata/remote/pi/sessions/demo/session.jsonl"), filepath.Join(home, ".pi/agent/sessions/demo/session.jsonl"))
	fakeBin := filepath.Join(tmp, "bin")
	if e := os.MkdirAll(fakeBin, 0700); e != nil {
		t.Fatal(e)
	}
	sshScript := "#!/bin/sh\n[ \"$5\" = bad ] && exit 255\nexport HOME=" + home + "\nexec /bin/sh\n"
	if e := os.WriteFile(filepath.Join(fakeBin, "ssh"), []byte(sshScript), 0700); e != nil {
		t.Fatal(e)
	}
	opencodeScript := `#!/bin/sh
case "$1" in
 --version) echo 'opencode synthetic';;
 db) echo '[{"id":"ses_child"},{"id":"ses_root"}]';;
 export) printf '{"id":"%s","synthetic":true}\n' "$2";;
 *) exit 2;;
esac
`
	if e := os.WriteFile(filepath.Join(fakeBin, "opencode"), []byte(opencodeScript), 0700); e != nil {
		t.Fatal(e)
	}
	clientHome := filepath.Join(tmp, "client-home")
	configHome := filepath.Join(tmp, "config")
	dataHome := filepath.Join(tmp, "data")
	cfg := filepath.Join(configHome, "glurp", "config.json")
	data := filepath.Join(dataHome, "glurp")
	env := append(os.Environ(), "PATH="+fakeBin+":"+os.Getenv("PATH"), "NO_COLOR=1", "HOME="+clientHome, "XDG_CONFIG_HOME="+configHome, "XDG_DATA_HOME="+dataHome)
	base := []string{}
	if out, e := run(t, env, bin, append(base, "host", "add", "good", "good")...); e != nil {
		t.Fatalf("add: %v %s", e, out)
	}
	if _, e := os.Stat(cfg); e != nil {
		t.Fatalf("default Glurp config was not created: %v", e)
	}
	if _, e := os.Stat(filepath.Join(configHome, "s"+"lurp", "config.json")); !os.IsNotExist(e) {
		t.Fatalf("legacy config path was unexpectedly used: %v", e)
	}
	if _, e := os.Stat(filepath.Join(dataHome, "s"+"lurp")); !os.IsNotExist(e) {
		t.Fatalf("legacy archive path was unexpectedly used: %v", e)
	}
	out, e := run(t, env, bin, append(base, "glurp", "good")...)
	if e != nil {
		t.Fatalf("collect: %v\n%s", e, out)
	}
	if !strings.Contains(out, "good/opencode: collected") || !strings.Contains(out, "failures=0") {
		t.Fatalf("unexpected report:\n%s", out)
	}
	paths := []string{"claude/demo/session.jsonl", "codex/sessions/2026/09/rollout.jsonl", "codex/archived_sessions/rollout.jsonl.zst", "pi/demo/session.jsonl", "opencode/sessions/ses_root.json", "opencode/sessions/ses_child.json"}
	expected := map[string][]byte{}
	for _, pair := range [][2]string{{"claude/demo/session.jsonl", "testdata/remote/claude/projects/demo/session.jsonl"}, {"codex/sessions/2026/09/rollout.jsonl", "testdata/remote/codex/sessions/2026/09/rollout.jsonl"}, {"codex/archived_sessions/rollout.jsonl.zst", "testdata/remote/codex/archived_sessions/rollout.jsonl.zst"}, {"pi/demo/session.jsonl", "testdata/remote/pi/sessions/demo/session.jsonl"}} {
		expected[pair[0]], _ = os.ReadFile(filepath.Join(repo, pair[1]))
	}
	expected["opencode/sessions/ses_root.json"] = []byte("{\"id\":\"ses_root\",\"synthetic\":true}\n")
	expected["opencode/sessions/ses_child.json"] = []byte("{\"id\":\"ses_child\",\"synthetic\":true}\n")
	mtimes := map[string]int64{}
	for _, p := range paths {
		full := filepath.Join(data, "hosts", "good", p)
		st, e := os.Stat(full)
		if e != nil {
			t.Errorf("missing %s: %v", p, e)
			continue
		}
		if got, _ := os.ReadFile(full); string(got) != string(expected[p]) {
			t.Errorf("content mismatch for %s", p)
		}
		mtimes[p] = st.ModTime().UnixNano()
	}
	out, e = run(t, env, bin, append(base, "glurp", "good")...)
	if e != nil {
		t.Fatalf("repeat: %v\n%s", e, out)
	}
	if !strings.Contains(out, "unchanged") {
		t.Fatalf("repeat not idempotent:\n%s", out)
	}
	for p, m := range mtimes {
		st, _ := os.Stat(filepath.Join(data, "hosts", "good", p))
		if st.ModTime().UnixNano() != m {
			t.Errorf("rewrote %s", p)
		}
	}
	remotePi := filepath.Join(home, ".pi/agent/sessions/demo/session.jsonl")
	if e := os.Remove(remotePi); e != nil {
		t.Fatal(e)
	}
	if out, e = run(t, env, bin, append(base, "glurp", "good", "--harness", "pi")...); e != nil {
		t.Fatalf("deletion retention run: %v\n%s", e, out)
	}
	if _, e = os.Stat(filepath.Join(data, "hosts", "good", "pi/demo/session.jsonl")); e != nil {
		t.Fatal("remote deletion removed local artifact")
	}
	remoteClaude := filepath.Join(home, ".claude/projects/demo/session.jsonl")
	if e = os.WriteFile(remoteClaude, []byte("updated synthetic transcript\n"), 0600); e != nil {
		t.Fatal(e)
	}
	if out, e = run(t, env, bin, append(base, "glurp", "good", "--harness", "claude")...); e != nil {
		t.Fatalf("update run: %v\n%s", e, out)
	}
	if got, _ := os.ReadFile(filepath.Join(data, "hosts", "good", "claude/demo/session.jsonl")); string(got) != "updated synthetic transcript\n" {
		t.Fatal("changed artifact was not replaced")
	}
	if out, e = run(t, env, bin, append(base, "host", "add", "bad", "bad")...); e != nil {
		t.Fatal(out, e)
	}
	out, e = run(t, env, bin, append(base, "glurp")...)
	if e == nil || !strings.Contains(out, "good/claude") || !strings.Contains(out, "bad/claude: failed") {
		t.Fatalf("partial failure behavior: err=%v\n%s", e, out)
	}
}
