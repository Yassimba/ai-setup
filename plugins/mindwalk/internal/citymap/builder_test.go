package citymap

import (
	"encoding/json"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/cosmtrek/mindwalk/internal/model"
)

func TestBuildIsDeterministic(t *testing.T) {
	root := t.TempDir()
	writeFile(t, root, "src/main.go", "package main\nfunc main() {}\n")
	writeFile(t, root, "README.md", "# Demo\n")
	runGit(t, root, "init")
	runGit(t, root, "add", ".")

	first, err := (Builder{}).Build(root, nil)
	if err != nil {
		t.Fatal(err)
	}
	second, err := (Builder{}).Build(root, nil)
	if err != nil {
		t.Fatal(err)
	}
	first.Repo.GeneratedAt = ""
	second.Repo.GeneratedAt = ""
	a, _ := json.Marshal(first)
	b, _ := json.Marshal(second)
	if string(a) != string(b) {
		t.Fatalf("citymap is not deterministic\nfirst=%s\nsecond=%s", a, b)
	}
	if len(first.Files) != 2 {
		t.Fatalf("files = %d", len(first.Files))
	}
	if first.Files[0].Rect.W <= 0 || first.Files[0].Rect.D <= 0 {
		t.Fatalf("empty rect: %#v", first.Files[0].Rect)
	}
}

func TestBuildIncludesUntrackedFiles(t *testing.T) {
	root := t.TempDir()
	writeFile(t, root, "tracked.go", "package main\n")
	runGit(t, root, "init")
	runGit(t, root, "add", "tracked.go")
	writeFile(t, root, "new.go", "package main\nfunc New() {}\n")

	city, err := (Builder{}).Build(root, nil)
	if err != nil {
		t.Fatal(err)
	}
	paths := map[string]bool{}
	for _, file := range city.Files {
		paths[file.Path] = true
	}
	if !paths["tracked.go"] || !paths["new.go"] {
		t.Fatalf("paths = %#v", paths)
	}
}

func TestBuildIsDeterministicAcrossNestedDirectories(t *testing.T) {
	root := t.TempDir()
	for d := 0; d < 12; d++ {
		for f := 0; f < 8; f++ {
			rel := filepath.Join("pkg", string(rune('a'+d)), "sub", string(rune('a'+f))+".go")
			writeFile(t, root, rel, strings.Repeat("package pkg\n", 1+d*f+f))
		}
	}
	runGit(t, root, "init")
	runGit(t, root, "add", ".")

	var first []byte
	for i := 0; i < 20; i++ {
		city, err := (Builder{}).Build(root, nil)
		if err != nil {
			t.Fatal(err)
		}
		city.Repo.GeneratedAt = ""
		data, _ := json.Marshal(city)
		if i == 0 {
			first = data
			continue
		}
		if string(data) != string(first) {
			t.Fatalf("citymap changed on run %d\nfirst=%s\ncurrent=%s", i, first, data)
		}
	}
}

func TestBuildSkipsWeakMissingTargetsButKeepsStrongGhosts(t *testing.T) {
	root := t.TempDir()
	writeFile(t, root, "tracked.go", "package main\n")
	runGit(t, root, "init")
	runGit(t, root, "add", "tracked.go")

	trace := &model.Trace{Events: []model.Event{{
		Targets: []model.Target{
			{Path: "missing-weak.go", Touch: "hit", Weak: true},
			{Path: "missing-strong.go", Touch: "edit"},
		},
	}}}
	city, err := (Builder{}).Build(root, trace)
	if err != nil {
		t.Fatal(err)
	}
	files := map[string]model.CityFile{}
	for _, file := range city.Files {
		files[file.Path] = file
	}
	if _, ok := files["missing-weak.go"]; ok {
		t.Fatalf("weak missing target became a city file: %#v", files["missing-weak.go"])
	}
	if file, ok := files["missing-strong.go"]; !ok || !file.Ghost {
		t.Fatalf("strong missing target did not become ghost: %#v", file)
	}
}

func TestSquarifiedLayoutAvoidsExtremeAspectRatios(t *testing.T) {
	root := t.TempDir()
	for i := 0; i < 80; i++ {
		writeFile(t, root, filepath.Join("pkg", "file"+string(rune('a'+i%26))+string(rune('a'+i/26))+".go"), "package pkg\nfunc X() {}\n")
	}
	runGit(t, root, "init")
	runGit(t, root, "add", ".")

	city, err := (Builder{}).Build(root, nil)
	if err != nil {
		t.Fatal(err)
	}
	maxRatio := 0.0
	for _, file := range city.Files {
		if file.Rect.W <= 0 || file.Rect.D <= 0 {
			t.Fatalf("empty rect for %s: %#v", file.Path, file.Rect)
		}
		ratio := math.Max(file.Rect.W/file.Rect.D, file.Rect.D/file.Rect.W)
		maxRatio = math.Max(maxRatio, ratio)
	}
	if maxRatio > 25 {
		t.Fatalf("max aspect ratio = %f", maxRatio)
	}
}

func TestLargeTextFileCountsLines(t *testing.T) {
	root := t.TempDir()
	const lines = 700000
	writeFile(t, root, "big.go", strings.Repeat("package main\n", lines))

	city, err := (Builder{}).Build(root, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(city.Files) != 1 {
		t.Fatalf("files = %#v", city.Files)
	}
	if city.Files[0].Lines != lines {
		t.Fatalf("lines = %d, want %d", city.Files[0].Lines, lines)
	}
}

func writeFile(t *testing.T, root, rel, content string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func runGit(t *testing.T, root string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = root
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}
