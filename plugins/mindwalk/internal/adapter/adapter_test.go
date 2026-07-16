package adapter

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/cosmtrek/mindwalk/internal/model"
)

func TestSessionKeyIsStableAndSourceSpecific(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.jsonl")
	first := SessionKey("codex", path)
	if second := SessionKey("codex", path); second != first {
		t.Fatalf("SessionKey changed: %q != %q", first, second)
	}
	if other := SessionKey("claude-code", path); other == first {
		t.Fatalf("SessionKey ignored harness: %q", other)
	}
	if other := SessionKey("codex", path+".copy"); other == first {
		t.Fatalf("SessionKey ignored path: %q", other)
	}
}

func TestBuildEventKeepsExecAggregatedAndFindsSingleCommandTarget(t *testing.T) {
	root := t.TempDir()
	writeAdapterTestFile(t, root, "README.md")
	source := `const r = await tools.exec_command({cmd:"sed -n '1,20p' README.md",workdir:` + jsonString(t, root) + `});`

	event := buildExecEvent(root, map[string]any{"_raw": source})
	if event.Tool != "exec" || event.Action != "read" {
		t.Fatalf("event = %#v", event)
	}
	if len(event.Targets) != 1 || event.Targets[0].Path != "README.md" || !event.Targets[0].Weak || event.Targets[0].Touch != "read" {
		t.Fatalf("targets = %#v", event.Targets)
	}
}

func TestBuildEventExtractsPromiseAllCommandTargets(t *testing.T) {
	root := t.TempDir()
	writeAdapterTestFile(t, root, "first/main.go")
	writeAdapterTestFile(t, root, "second/main.go")
	first := filepath.Join(root, "first")
	second := filepath.Join(root, "second")
	source := `const rs = await Promise.all([
  tools.exec_command({cmd:"sed -n '1,20p' main.go",workdir:` + jsonString(t, first) + `}),
  tools.exec_command({"cmd":"rg TODO main.go","workdir":` + jsonString(t, second) + `})
]);`

	event := buildExecEvent(root, map[string]any{"code": source})
	if event.Tool != "exec" || event.Action != "exec" {
		t.Fatalf("event = %#v", event)
	}
	if len(event.Targets) != 2 {
		t.Fatalf("targets = %#v", event.Targets)
	}
	want := []struct {
		path  string
		touch string
	}{{"first/main.go", "read"}, {"second/main.go", "hit"}}
	for i, target := range event.Targets {
		if target.Path != want[i].path || target.Touch != want[i].touch || !target.Weak {
			t.Fatalf("target %d = %#v", i, target)
		}
	}
}

func TestBuildEventDecodesEscapedExecStrings(t *testing.T) {
	root := t.TempDir()
	workdir := filepath.Join(root, `quoted"dir`)
	writeAdapterTestFile(t, workdir, "src/main.go")
	command := `sed -n "1,20p" src/main.go`
	source := `tools.exec_command({cmd:` + jsonString(t, command) + `,workdir:` + jsonString(t, workdir) + `});`

	event := buildExecEvent(root, map[string]any{"script": source})
	if len(event.Targets) != 1 || event.Targets[0].Path != `quoted"dir/src/main.go` || !event.Targets[0].Weak {
		t.Fatalf("targets = %#v", event.Targets)
	}
}

func TestBuildEventAcceptsWorkdirBeforeCommand(t *testing.T) {
	root := t.TempDir()
	workdir := filepath.Join(root, "sub")
	writeAdapterTestFile(t, workdir, "main.go")
	source := `tools.exec_command({workdir:` + jsonString(t, workdir) + `,cmd:"sed main.go"});`

	event := buildExecEvent(root, map[string]any{"_raw": source})
	if len(event.Targets) != 1 || event.Targets[0].Path != "sub/main.go" {
		t.Fatalf("targets = %#v", event.Targets)
	}
}

func TestBuildEventExecActionRequiresEveryCommandToVerify(t *testing.T) {
	tests := []struct {
		name   string
		source string
		want   string
	}{
		{
			name:   "all verification commands",
			source: `Promise.all([tools.exec_command({cmd:"go test ./..."}), tools.exec_command({cmd:"make test"})])`,
			want:   "verify",
		},
		{
			name:   "verification and ordinary command",
			source: `Promise.all([tools.exec_command({cmd:"go test ./..."}), tools.exec_command({cmd:"sed -n '1,20p' README.md"})])`,
			want:   "exec",
		},
		{
			name:   "verification and another tool",
			source: `Promise.all([tools.exec_command({cmd:"go test ./..."}), tools.apply_patch("*** Begin Patch")])`,
			want:   "exec",
		},
		{
			name:   "dynamic command",
			source: `tools.exec_command({cmd,workdir:"/tmp"})`,
			want:   "exec",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			event := buildExecEvent(t.TempDir(), map[string]any{"_raw": tt.source})
			if event.Tool != "exec" || event.Action != tt.want {
				t.Fatalf("event = %#v, want action %q", event, tt.want)
			}
		})
	}
}

func TestBuildEventClassifiesBashSearchCommands(t *testing.T) {
	tests := []struct {
		command string
		want    string
	}{
		{`grep -rn "Pair with AI" src --include="*.ts" | head -5`, "search"},
		{`find . -name "*.go" | wc -l`, "search"},
		{`cd /repo && rg TODO internal`, "search"},
		{`git grep -n hook`, "search"},
		{`FOO=1 grep -c x file.txt 2>/dev/null`, "search"},
		{`ls web/src`, "search"},
		{`grep x file > out.txt`, "exec"},
		{`find . -name "*.tmp" -delete`, "exec"},
		{`grep x file && rm file`, "exec"},
		{`npm install 2>&1 | tail -3`, "exec"},
		{`echo done`, "exec"},
		{`go test ./... | tail -5`, "verify"},
		{`cat internal/model/stats.go`, "read"},
		{`sed -n '1,240p' web/src/ui/Hud.tsx`, "read"},
		{`nl -ba main.go | head -50`, "read"},
		{`head -n 20 Makefile`, "read"},
		{`sed -i '' 's/a/b/g' main.go`, "exec"},
		{`cat notes.md > backup.md`, "exec"},
		{`cat main.go && rm main.go`, "exec"},
	}

	for _, tt := range tests {
		t.Run(tt.command, func(t *testing.T) {
			trace := &model.Trace{Session: model.TraceSession{Cwd: t.TempDir()}}
			event := BuildEvent(trace, ToolCall{Name: "Bash", Input: map[string]any{"command": tt.command}}, ToolResult{})
			if event.Action != tt.want {
				t.Fatalf("action(%q) = %q, want %q", tt.command, event.Action, tt.want)
			}
		})
	}
}

func TestBuildEventExecActionAllSearchCommands(t *testing.T) {
	source := `Promise.all([tools.exec_command({cmd:"rg TODO main.go"}), tools.exec_command({cmd:"ls src"})])`
	event := buildExecEvent(t.TempDir(), map[string]any{"_raw": source})
	if event.Tool != "exec" || event.Action != "search" {
		t.Fatalf("event = %#v, want action %q", event, "search")
	}
}

func TestBuildEventExecActionAllReadCommands(t *testing.T) {
	source := `Promise.all([tools.exec_command({cmd:"sed -n '1,20p' main.go"}), tools.exec_command({cmd:"cat README.md"})])`
	event := buildExecEvent(t.TempDir(), map[string]any{"_raw": source})
	if event.Tool != "exec" || event.Action != "read" {
		t.Fatalf("event = %#v, want action %q", event, "read")
	}
}

func TestCommandReadPaths(t *testing.T) {
	tests := []struct {
		command string
		want    []string
	}{
		{`sed -n '1,240p' internal/adapter/adapter.go`, []string{"internal/adapter/adapter.go"}},
		{`cat a.go b.go`, []string{"a.go", "b.go"}},
		{`head -n 20 Makefile`, []string{"Makefile"}},
		{`tail -f logs/app.log`, []string{"logs/app.log"}},
		{`cat src/main.go | rg TODO`, []string{"src/main.go"}},
		{`sed -i '' 's/x/y/' a.go`, nil},
		{`cat file.go > copy.go`, nil},
		{`grep -rn TODO src`, nil},
		{`cat *.go`, nil},
		{`cat <<EOF > notes.md`, nil},
	}

	for _, tt := range tests {
		t.Run(tt.command, func(t *testing.T) {
			got := commandReadPaths(tt.command)
			if len(got) != len(tt.want) {
				t.Fatalf("commandReadPaths(%q) = %#v, want %#v", tt.command, got, tt.want)
			}
			for i := range got {
				if got[i] != tt.want[i] {
					t.Fatalf("commandReadPaths(%q) = %#v, want %#v", tt.command, got, tt.want)
				}
			}
		})
	}
}

func TestBuildEventDoesNotPairDistantExecWorkdir(t *testing.T) {
	root := t.TempDir()
	writeAdapterTestFile(t, root, "README.md")
	source := `tools.exec_command({cmd:"sed README.md"}); const metadata = {workdir:"/tmp/not-the-command-workdir"};`

	event := buildExecEvent(root, map[string]any{"_raw": source})
	if len(event.Targets) != 1 || event.Targets[0].Path != "README.md" || !event.Targets[0].Weak {
		t.Fatalf("targets = %#v", event.Targets)
	}
	if len(event.Outside) != 0 {
		t.Fatalf("outside = %#v", event.Outside)
	}
}

func TestBuildEventIgnoresExecExamplesInStringsAndComments(t *testing.T) {
	root := t.TempDir()
	writeAdapterTestFile(t, root, "README.md")
	source := "const quoted = 'tools.exec_command({cmd:\"go test ./...\"})';\n" +
		"const template = `tools.exec_command({cmd:\"sed README.md\"})`;\n" +
		"// tools.exec_command({cmd:\"sed README.md\"})\n" +
		"/* tools.exec_command({cmd:\"sed README.md\"}) */\n" +
		"text(quoted + template);"

	event := buildExecEvent(root, map[string]any{"_raw": source})
	if event.Action != "exec" || len(event.Targets) != 0 {
		t.Fatalf("event = %#v", event)
	}
}

func TestBuildEventExtractsJSReplTargets(t *testing.T) {
	root := t.TempDir()
	writeAdapterTestFile(t, root, "packages/db/src/index.ts")
	code := `const db = await import("./packages/db/src/index.ts")`

	for _, key := range []string{"code", "_raw"} {
		t.Run(key, func(t *testing.T) {
			trace := &model.Trace{Session: model.TraceSession{Cwd: root}}
			event := BuildEvent(trace, ToolCall{Name: "js_repl", Input: map[string]any{key: code}}, ToolResult{})
			if event.Tool != "js_repl" || event.Action != "exec" {
				t.Fatalf("event = %#v", event)
			}
			if len(event.Targets) != 1 || event.Targets[0].Path != "packages/db/src/index.ts" || !event.Targets[0].Weak {
				t.Fatalf("targets = %#v", event.Targets)
			}
		})
	}
}

func buildExecEvent(cwd string, input map[string]any) model.Event {
	trace := &model.Trace{Session: model.TraceSession{Cwd: cwd}}
	return BuildEvent(trace, ToolCall{Name: "exec", Input: input}, ToolResult{})
}

func writeAdapterTestFile(t *testing.T, root, path string) {
	t.Helper()
	fullPath := filepath.Join(root, filepath.FromSlash(path))
	if err := os.MkdirAll(filepath.Dir(fullPath), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(fullPath, []byte("test\n"), 0o644); err != nil {
		t.Fatal(err)
	}
}

func jsonString(t *testing.T, value string) string {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(encoded)
}
