package citymap

import (
	"bytes"
	"errors"
	"io"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/cosmtrek/mindwalk/internal/model"
)

type Builder struct{}

func (Builder) Build(repoRoot string, trace *model.Trace) (*model.CityMap, error) {
	root, err := filepath.Abs(repoRoot)
	if err != nil {
		return nil, err
	}
	files, err := listFiles(root)
	if err != nil {
		return nil, err
	}

	seen := map[string]bool{}
	cityFiles := make([]model.CityFile, 0, len(files))
	for _, rel := range files {
		meta, err := inspectFile(root, rel)
		if err != nil {
			continue
		}
		seen[rel] = true
		cityFiles = append(cityFiles, meta)
	}

	if trace != nil {
		for _, event := range trace.Events {
			for _, target := range event.Targets {
				if target.Path == "" || seen[target.Path] {
					continue
				}
				if target.Weak {
					continue
				}
				seen[target.Path] = true
				cityFiles = append(cityFiles, model.CityFile{
					Path:  target.Path,
					Dir:   filepath.ToSlash(filepath.Dir(target.Path)),
					Lang:  langForPath(target.Path),
					Ghost: true,
				})
			}
		}
	}

	sort.Slice(cityFiles, func(i, j int) bool {
		return cityFiles[i].Path < cityFiles[j].Path
	})
	for i := range cityFiles {
		cityFiles[i].ID = i
	}

	rootNode := buildTree(cityFiles)
	dirs := make([]model.CityDir, 0)
	layoutNode(rootNode, model.Rect{X: 0, Z: 0, W: 120, D: 120}, &cityFiles, &dirs)

	commit, dirty := repoState(root)
	return &model.CityMap{
		Version: 1,
		Repo: model.RepoMeta{
			Root:        root,
			Commit:      commit,
			Dirty:       dirty,
			GeneratedAt: time.Now().UTC().Format(time.RFC3339),
		},
		Files: cityFiles,
		Dirs:  dirs,
		Layout: model.LayoutMeta{
			Algorithm: "squarified-treemap-v1",
			Weight:    "sqrt(max(lines, bytes/4096, 16))",
		},
	}, nil
}

func listFiles(root string) ([]string, error) {
	cmd := exec.Command("git", "-C", root, "ls-files", "-co", "--exclude-standard", "-z")
	out, err := cmd.Output()
	if err == nil && len(out) > 0 {
		parts := bytes.Split(out, []byte{0})
		files := make([]string, 0, len(parts))
		for _, part := range parts {
			if len(part) == 0 {
				continue
			}
			files = append(files, filepath.ToSlash(string(part)))
		}
		return files, nil
	}

	var files []string
	err = filepath.WalkDir(root, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			switch entry.Name() {
			case ".git", "node_modules", ".venv", "dist", "build":
				return filepath.SkipDir
			}
			return nil
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		files = append(files, filepath.ToSlash(rel))
		return nil
	})
	if err != nil {
		return nil, err
	}
	if len(files) == 0 {
		return nil, errors.New("no files found")
	}
	sort.Strings(files)
	return files, nil
}

func inspectFile(root, rel string) (model.CityFile, error) {
	abs := filepath.Join(root, filepath.FromSlash(rel))
	info, err := os.Stat(abs)
	if err != nil {
		return model.CityFile{}, err
	}
	lines := 1
	if !isBinaryLike(abs, rel) {
		var err error
		lines, err = countLines(abs)
		if err != nil {
			lines = 0
		}
	}
	dir := filepath.ToSlash(filepath.Dir(rel))
	if dir == "." {
		dir = ""
	}
	return model.CityFile{
		Path:  filepath.ToSlash(rel),
		Dir:   dir,
		Lines: lines,
		Bytes: info.Size(),
		Lang:  langForPath(rel),
	}, nil
}

func isBinaryLike(abs, path string) bool {
	ext := strings.ToLower(filepath.Ext(path))
	switch ext {
	case ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".zip", ".gz", ".tgz", ".mp4", ".mov", ".mp3", ".woff", ".woff2", ".ttf", ".otf":
		return true
	}
	f, err := os.Open(abs)
	if err != nil {
		return false
	}
	defer f.Close()
	buf := make([]byte, 8192)
	n, err := f.Read(buf)
	if err != nil && n == 0 {
		return false
	}
	return bytes.Contains(buf[:n], []byte{0})
}

func countLines(path string) (int, error) {
	f, err := os.Open(path)
	if err != nil {
		return 0, err
	}
	defer f.Close()

	lines := 0
	hasBytes := false
	var last byte
	buf := make([]byte, 64*1024)
	for {
		n, err := f.Read(buf)
		if n > 0 {
			hasBytes = true
			last = buf[n-1]
			lines += bytes.Count(buf[:n], []byte{'\n'})
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return lines, err
		}
	}
	if hasBytes && last != '\n' {
		lines++
	}
	return lines, nil
}

func langForPath(path string) string {
	ext := strings.TrimPrefix(strings.ToLower(filepath.Ext(path)), ".")
	switch ext {
	case "go":
		return "go"
	case "ts", "tsx":
		return "typescript"
	case "js", "jsx":
		return "javascript"
	case "md", "mdx":
		return "markdown"
	case "py":
		return "python"
	case "json":
		return "json"
	case "yaml", "yml":
		return "yaml"
	case "css":
		return "css"
	case "html":
		return "html"
	case "":
		return "text"
	default:
		return ext
	}
}

func repoState(root string) (string, bool) {
	commitBytes, err := exec.Command("git", "-C", root, "rev-parse", "--short", "HEAD").Output()
	commit := ""
	if err == nil {
		commit = strings.TrimSpace(string(commitBytes))
	}

	statusBytes, err := exec.Command("git", "-C", root, "status", "--porcelain").Output()
	dirty := err == nil && len(bytes.TrimSpace(statusBytes)) > 0
	return commit, dirty
}

type node struct {
	name      string
	path      string
	children  map[string]*node
	files     []int
	weight    float64
	rect      model.Rect
	fileCount int
	lines     int
}

func buildTree(files []model.CityFile) *node {
	root := &node{name: "", children: map[string]*node{}}
	for i, file := range files {
		parts := splitPath(file.Path)
		cur := root
		for _, part := range parts[:len(parts)-1] {
			next := cur.children[part]
			if next == nil {
				nextPath := part
				if cur.path != "" {
					nextPath = cur.path + "/" + part
				}
				next = &node{name: part, path: nextPath, children: map[string]*node{}}
				cur.children[part] = next
			}
			cur = next
		}
		cur.files = append(cur.files, i)
	}
	computeWeight(root, files)
	return root
}

func splitPath(path string) []string {
	parts := strings.Split(filepath.ToSlash(path), "/")
	out := parts[:0]
	for _, part := range parts {
		if part != "" && part != "." {
			out = append(out, part)
		}
	}
	if len(out) == 0 {
		return []string{path}
	}
	return out
}

func computeWeight(n *node, files []model.CityFile) float64 {
	n.weight = 0
	for _, idx := range n.files {
		n.weight += fileWeight(files[idx])
		n.fileCount++
		n.lines += files[idx].Lines
	}
	for _, child := range sortedChildren(n.children) {
		n.weight += computeWeight(child, files)
		n.fileCount += child.fileCount
		n.lines += child.lines
	}
	if n.weight <= 0 {
		n.weight = 1
	}
	return n.weight
}

type layoutItem struct {
	name   string
	kind   string
	idx    int
	node   *node
	weight float64
	area   float64
}

func layoutNode(n *node, rect model.Rect, files *[]model.CityFile, dirs *[]model.CityDir) {
	n.rect = rect
	if n.path != "" {
		*dirs = append(*dirs, model.CityDir{
			Path:      n.path,
			Depth:     len(strings.Split(n.path, "/")),
			Rect:      n.rect,
			FileCount: n.fileCount,
			Lines:     n.lines,
		})
	}

	items := make([]layoutItem, 0, len(n.children)+len(n.files))
	for _, child := range sortedChildren(n.children) {
		items = append(items, layoutItem{name: child.name, kind: "dir", node: child, weight: child.weight})
	}
	for _, idx := range n.files {
		items = append(items, layoutItem{name: (*files)[idx].Path, kind: "file", idx: idx, weight: fileWeight((*files)[idx])})
	}
	sort.Slice(items, func(i, j int) bool {
		if items[i].weight == items[j].weight {
			return items[i].name < items[j].name
		}
		return items[i].weight > items[j].weight
	})

	total := 0.0
	for _, item := range items {
		total += item.weight
	}
	if total <= 0 {
		return
	}

	scale := rect.W * rect.D / total
	for i := range items {
		items[i].area = items[i].weight * scale
	}
	for _, placed := range squarify(rect, items) {
		childRect := capAspect(inset(placed.rect, 0.08), 40)
		item := placed.item
		if item.kind == "dir" {
			layoutNode(item.node, childRect, files, dirs)
		} else {
			(*files)[item.idx].Rect = childRect
		}
	}
}

func sortedChildren(children map[string]*node) []*node {
	names := make([]string, 0, len(children))
	for name := range children {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make([]*node, 0, len(names))
	for _, name := range names {
		out = append(out, children[name])
	}
	return out
}

func capAspect(rect model.Rect, maxRatio float64) model.Rect {
	if rect.W <= 0 || rect.D <= 0 || maxRatio <= 1 {
		return rect
	}
	if rect.W/rect.D > maxRatio {
		newW := rect.D * maxRatio
		rect.X += (rect.W - newW) / 2
		rect.W = newW
	} else if rect.D/rect.W > maxRatio {
		newD := rect.W * maxRatio
		rect.Z += (rect.D - newD) / 2
		rect.D = newD
	}
	return rect
}

func fileWeight(file model.CityFile) float64 {
	units := float64(file.Lines)
	if byteUnits := float64(file.Bytes) / 4096.0; byteUnits > units {
		units = byteUnits
	}
	if units < 16 {
		units = 16
	}
	return math.Sqrt(units)
}

type placedItem struct {
	item layoutItem
	rect model.Rect
}

func squarify(rect model.Rect, items []layoutItem) []placedItem {
	remaining := rect
	var placed []placedItem
	var row []layoutItem
	for idx, item := range items {
		if item.area <= 0 {
			continue
		}
		side := math.Min(remaining.W, remaining.D)
		if len(row) < 2 || idx == len(items)-1 || worstAspect(append(row, item), side) <= worstAspect(row, side) {
			row = append(row, item)
			continue
		}
		var rowPlaced []placedItem
		rowPlaced, remaining = layoutRow(remaining, row)
		placed = append(placed, rowPlaced...)
		row = []layoutItem{item}
	}
	if len(row) > 0 {
		rowPlaced, _ := layoutRow(remaining, row)
		placed = append(placed, rowPlaced...)
	}
	return placed
}

func worstAspect(row []layoutItem, side float64) float64 {
	if len(row) == 0 || side <= 0 {
		return math.Inf(1)
	}
	sum := 0.0
	minArea := math.Inf(1)
	maxArea := 0.0
	for _, item := range row {
		sum += item.area
		minArea = math.Min(minArea, item.area)
		maxArea = math.Max(maxArea, item.area)
	}
	if sum <= 0 || minArea <= 0 {
		return math.Inf(1)
	}
	side2 := side * side
	sum2 := sum * sum
	return math.Max(side2*maxArea/sum2, sum2/(side2*minArea))
}

func layoutRow(rect model.Rect, row []layoutItem) ([]placedItem, model.Rect) {
	sum := 0.0
	for _, item := range row {
		sum += item.area
	}
	if sum <= 0 {
		return nil, rect
	}
	placed := make([]placedItem, 0, len(row))
	if rect.W >= rect.D {
		rowD := sum / rect.W
		x := rect.X
		for i, item := range row {
			w := item.area / rowD
			if i == len(row)-1 {
				w = rect.X + rect.W - x
			}
			placed = append(placed, placedItem{item: item, rect: model.Rect{X: x, Z: rect.Z, W: w, D: rowD}})
			x += w
		}
		rect.Z += rowD
		rect.D -= rowD
	} else {
		rowW := sum / rect.D
		z := rect.Z
		for i, item := range row {
			d := item.area / rowW
			if i == len(row)-1 {
				d = rect.Z + rect.D - z
			}
			placed = append(placed, placedItem{item: item, rect: model.Rect{X: rect.X, Z: z, W: rowW, D: d}})
			z += d
		}
		rect.X += rowW
		rect.W -= rowW
	}
	if rect.W < 0 {
		rect.W = 0
	}
	if rect.D < 0 {
		rect.D = 0
	}
	return placed, rect
}

func inset(rect model.Rect, pad float64) model.Rect {
	if rect.W > pad*2 {
		rect.X += pad
		rect.W -= pad * 2
	}
	if rect.D > pad*2 {
		rect.Z += pad
		rect.D -= pad * 2
	}
	return rect
}
