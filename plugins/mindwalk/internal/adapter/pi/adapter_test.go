package pi

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestParsePiSession(t *testing.T) {
	repo := t.TempDir()
	if err := os.WriteFile(filepath.Join(repo, "read.go"), []byte("package read\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	session := filepath.Join(t.TempDir(), "session.jsonl")
	writePiSession(t, session,
		fmt.Sprintf(`{"type":"session","version":3,"id":"pi-demo","timestamp":"2026-07-13T10:00:00Z","cwd":%q}`, repo),
		`{"type":"model_change","id":"m1","parentId":null,"timestamp":"2026-07-13T10:00:01Z","modelId":"gpt-test"}`,
		`{"type":"message","id":"u1","parentId":"m1","timestamp":"2026-07-13T10:00:02Z","message":{"role":"user","content":"inspect"}}`,
		`{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-07-13T10:00:03Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"r1","name":"read","arguments":{"path":"read.go","offset":1,"limit":2}},{"type":"toolCall","id":"w1","name":"write","arguments":{"path":"new.go","content":"package new"}}]}}`,
		`{"type":"message","id":"r2","parentId":"a1","timestamp":"2026-07-13T10:00:04Z","message":{"role":"toolResult","toolCallId":"r1","toolName":"read","content":[{"type":"text","text":"package read"}],"isError":false}}`,
		`{"type":"message","id":"w2","parentId":"r2","timestamp":"2026-07-13T10:00:05Z","message":{"role":"toolResult","toolCallId":"w1","toolName":"write","content":[{"type":"text","text":"wrote new.go"}],"isError":false}}`,
		`{"type":"compaction","id":"c1","parentId":"w2","timestamp":"2026-07-13T10:00:06Z","summary":"summary","firstKeptEntryId":"u1","tokensBefore":100}`,
	)

	trace, err := (Adapter{}).Parse(session)
	if err != nil {
		t.Fatal(err)
	}
	if trace.Session.ID != "pi-demo" || trace.Session.Harness != "pi" || trace.Session.Model != "gpt-test" {
		t.Fatalf("session = %#v", trace.Session)
	}
	if len(trace.Events) != 2 {
		t.Fatalf("events = %#v", trace.Events)
	}
	if got := trace.Events[0]; got.Tool != "Read" || got.Action != "read" || len(got.Targets) != 1 || got.Targets[0].Path != "read.go" {
		t.Fatalf("read event = %#v", got)
	}
	if got := trace.Events[1]; got.Tool != "Write" || got.Action != "edit" || len(got.Targets) != 1 || got.Targets[0].Path != "new.go" {
		t.Fatalf("write event = %#v", got)
	}
	if len(trace.Marks) != 2 || trace.Marks[0].Type != "user-message" || trace.Marks[1].Type != "compaction" {
		t.Fatalf("marks = %#v", trace.Marks)
	}
}

func TestListSessionsHidesNestedSubagentRuns(t *testing.T) {
	root := t.TempDir()
	project := filepath.Join(root, "project")
	if err := os.MkdirAll(filepath.Join(project, "main", "child", "run-0"), 0o700); err != nil {
		t.Fatal(err)
	}
	mainSession := filepath.Join(project, "main.jsonl")
	childSession := filepath.Join(project, "main", "child", "run-0", "session.jsonl")
	writePiSession(t, mainSession,
		`{"type":"session","version":3,"id":"main","timestamp":"2026-07-13T10:00:00Z","cwd":"/tmp"}`,
	)
	writePiSession(t, childSession,
		`{"type":"session","version":3,"id":"child","timestamp":"2026-07-13T10:00:01Z","cwd":"/tmp"}`,
	)

	child, err := (Adapter{Dir: root}).Summarize(childSession)
	if err != nil || !child.Auxiliary {
		t.Fatalf("child = %#v, err = %v", child, err)
	}
	sessions, err := (Adapter{Dir: root}).ListSessions()
	if err != nil {
		t.Fatal(err)
	}
	if len(sessions) != 1 || sessions[0].ID != "main" {
		t.Fatalf("sessions = %#v", sessions)
	}
}

func TestSummarizeRejectsNonPiJSONL(t *testing.T) {
	path := filepath.Join(t.TempDir(), "other.jsonl")
	writePiSession(t, path, `{"type":"message","timestamp":"2026-07-13T10:00:00Z"}`)
	if _, err := (Adapter{}).Summarize(path); err == nil {
		t.Fatal("expected non-Pi JSONL to be rejected")
	}
}

func writePiSession(t *testing.T, path string, lines ...string) {
	t.Helper()
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	for _, line := range lines {
		if _, err := fmt.Fprintln(file, line); err != nil {
			t.Fatal(err)
		}
	}
}
