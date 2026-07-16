package pi

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/cosmtrek/mindwalk/internal/adapter"
	"github.com/cosmtrek/mindwalk/internal/model"
)

type Adapter struct {
	Dir string
}

func DefaultDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".pi", "agent", "sessions")
}

func (a Adapter) Harness() string {
	return "pi"
}

func (a Adapter) SessionDir() string {
	if a.Dir != "" {
		return a.Dir
	}
	return DefaultDir()
}

func (a Adapter) ListSessions() ([]model.SessionMeta, error) {
	dir := a.SessionDir()
	if info, err := os.Stat(dir); err != nil || !info.IsDir() {
		return nil, nil
	}

	var metas []model.SessionMeta
	err := filepath.WalkDir(dir, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return nil
		}
		if entry.IsDir() {
			return nil
		}
		if filepath.Ext(path) != ".jsonl" {
			return nil
		}
		meta, err := a.Summarize(path)
		if err == nil && !meta.Auxiliary {
			metas = append(metas, meta)
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	sort.Slice(metas, func(i, j int) bool {
		return metas[i].EndedAt > metas[j].EndedAt
	})
	return metas, nil
}

func (a Adapter) Summarize(path string) (model.SessionMeta, error) {
	f, err := os.Open(path)
	if err != nil {
		return model.SessionMeta{}, err
	}
	defer f.Close()

	id := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	meta := model.SessionMeta{
		Key:       adapter.SessionKey(a.Harness(), path),
		ID:        id,
		Harness:   a.Harness(),
		Path:      path,
		Auxiliary: isAuxiliarySession(a.SessionDir(), path),
	}
	sawSession := false
	seenCalls := map[string]bool{}

	err = adapter.ReadJSONLines(f, func(data []byte) {
		var line rawLine
		if json.Unmarshal(data, &line) != nil {
			return
		}
		if line.Timestamp != "" {
			if meta.StartedAt == "" {
				meta.StartedAt = line.Timestamp
			}
			meta.EndedAt = line.Timestamp
		}
		switch line.Type {
		case "session":
			sawSession = true
			if line.ID != "" {
				meta.ID = line.ID
			}
			if line.Cwd != "" && meta.Cwd == "" {
				meta.Cwd = line.Cwd
			}
		case "model_change":
			if meta.Model == "" && line.ModelID != "" {
				meta.Model = line.ModelID
			}
		case "session_info":
			if line.Name != "" {
				meta.Title = line.Name
			}
		case "message":
			msg := line.Message
			if msg == nil {
				return
			}
			if msg.Role == "assistant" {
				if meta.Model == "" && msg.Model != "" {
					meta.Model = msg.Model
				}
				for _, item := range msg.Content {
					if item.Type == "toolCall" && item.ID != "" {
						if !seenCalls[item.ID] {
							seenCalls[item.ID] = true
							meta.EventCount++
						}
					}
				}
			}
		}
	})
	if meta.Title == "" {
		meta.Title = filepath.Base(path)
	}
	if !sawSession {
		return model.SessionMeta{}, fmt.Errorf("not a Pi session: %s", path)
	}
	return meta, err
}

func (a Adapter) Parse(path string) (*model.Trace, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	id := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	trace := &model.Trace{
		Version: 1,
		Session: model.TraceSession{
			ID:      id,
			Harness: a.Harness(),
			Path:    path,
		},
		Events: []model.Event{},
		Marks:  []model.Mark{},
	}

	sawSession := false
	calls := map[string]adapter.ToolCall{}
	callOrder := []string{}
	results := map[string]adapter.ToolResult{}

	err = adapter.ReadJSONLines(f, func(data []byte) {
		var line rawLine
		if json.Unmarshal(data, &line) != nil {
			return
		}
		if line.Timestamp != "" {
			applyLineTime(trace, line.Timestamp)
		}
		switch line.Type {
		case "session":
			sawSession = true
			if line.ID != "" {
				trace.Session.ID = line.ID
			}
			if line.Cwd != "" {
				trace.Session.Cwd = line.Cwd
			}
		case "model_change":
			if trace.Session.Model == "" && line.ModelID != "" {
				trace.Session.Model = line.ModelID
			}
		case "session_info":
			if line.Name != "" {
				trace.Session.Title = line.Name
			}
		case "compaction":
			trace.Marks = append(trace.Marks, model.Mark{Seq: len(callOrder), Type: "compaction"})
		case "message":
			msg := line.Message
			if msg == nil {
				return
			}
			switch msg.Role {
			case "user":
				trace.Marks = append(trace.Marks, model.Mark{
					Seq:  len(callOrder),
					Type: "user-message",
				})
			case "assistant":
				if trace.Session.Model == "" && msg.Model != "" {
					trace.Session.Model = msg.Model
				}
				for _, item := range msg.Content {
					if item.Type == "toolCall" {
						if item.Name == "subagent" {
							trace.Marks = append(trace.Marks, model.Mark{Seq: len(callOrder), Type: "subagent", Note: item.Name})
						}
						call := adapter.ToolCall{
							ID:        item.ID,
							Name:      normalizeToolName(item.Name),
							Input:     normalizeInput(item.Name, item.Arguments),
							Timestamp: line.Timestamp,
						}
						if _, exists := calls[call.ID]; exists {
							return
						}
						calls[call.ID] = call
						callOrder = append(callOrder, call.ID)
					}
				}
			case "toolResult":
				result := adapter.ToolResult{
					Content: contentItemsToString(msg.Content),
					IsError: msg.IsError,
				}
				if msg.ToolCallID != "" {
					if _, exists := results[msg.ToolCallID]; exists {
						return
					}
					results[msg.ToolCallID] = result
				}
			}
		}
	})

	for _, callID := range callOrder {
		call := calls[callID]
		result := results[callID]
		trace.Events = append(trace.Events, adapter.BuildEvent(trace, call, result))
	}
	for i := range trace.Events {
		trace.Events[i].Seq = i
	}
	if trace.Session.Title == "" {
		trace.Session.Title = filepath.Base(path)
	}
	trace.Session.EventCount = len(trace.Events)
	trace.Stats = model.ComputeStats(trace, 0, model.ObservabilityEstimated)
	if !sawSession {
		return nil, fmt.Errorf("not a Pi session: %s", path)
	}
	return trace, err
}

type rawLine struct {
	Type      string      `json:"type"`
	ID        string      `json:"id"`
	ParentID  string      `json:"parentId"`
	Timestamp string      `json:"timestamp"`
	Cwd       string      `json:"cwd"`
	ModelID   string      `json:"modelId"`
	Provider  string      `json:"provider"`
	Name      string      `json:"name"`
	Message   *rawMessage `json:"message"`
}

type rawMessage struct {
	Role       string         `json:"role"`
	Model      string         `json:"model"`
	Content    rawContentList `json:"content"`
	ToolCallID string         `json:"toolCallId"`
	ToolName   string         `json:"toolName"`
	IsError    bool           `json:"isError"`
	Timestamp  int64          `json:"timestamp"`
}

type rawContentList []rawContentItem

func (content *rawContentList) UnmarshalJSON(data []byte) error {
	if len(data) == 0 || string(data) == "null" {
		return nil
	}
	if data[0] == '"' {
		var text string
		if err := json.Unmarshal(data, &text); err != nil {
			return err
		}
		*content = rawContentList{{Type: "text", Text: text}}
		return nil
	}
	var items []rawContentItem
	if err := json.Unmarshal(data, &items); err != nil {
		return err
	}
	*content = items
	return nil
}

type rawContentItem struct {
	Type      string         `json:"type"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Text      string         `json:"text"`
	Thinking  string         `json:"thinking"`
	Arguments map[string]any `json:"arguments"`
}

func isAuxiliarySession(root, path string) bool {
	relative, err := filepath.Rel(root, path)
	if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return false
	}
	parts := strings.Split(filepath.Clean(relative), string(filepath.Separator))
	for i := 1; i < len(parts)-1; i++ {
		parentSession := filepath.Join(append([]string{root}, parts[:i+1]...)...) + ".jsonl"
		if info, err := os.Stat(parentSession); err == nil && !info.IsDir() {
			return true
		}
	}
	return false
}

func applyLineTime(trace *model.Trace, ts string) {
	if ts == "" {
		return
	}
	if trace.Session.StartedAt == "" {
		trace.Session.StartedAt = ts
	}
	trace.Session.EndedAt = ts
}

func contentItemsToString(items []rawContentItem) string {
	parts := make([]string, 0, len(items))
	for _, item := range items {
		if item.Type == "text" && item.Text != "" {
			parts = append(parts, item.Text)
		}
	}
	return strings.Join(parts, "\n")
}

// normalizeInput maps Pi tool arguments to the canonical keys expected by
// adapter.targetsFor (e.g. Pi uses "path" while adapter expects "file_path" for Read).
func normalizeInput(tool string, args map[string]any) map[string]any {
	if args == nil {
		return map[string]any{}
	}
	out := make(map[string]any, len(args))
	for k, v := range args {
		out[k] = v
	}
	switch tool {
	case "read", "write", "edit":
		if path, ok := out["path"]; ok {
			if _, exists := out["file_path"]; !exists {
				out["file_path"] = path
			}
		}
	case "find":
		if pattern, ok := out["pattern"]; ok {
			if _, exists := out["path"]; !exists {
				out["path"] = pattern
			}
		}
	}
	return out
}

// normalizeToolName maps Pi tool names to the canonical names expected by
// adapter.actionFor / adapter.targetsFor.
func normalizeToolName(name string) string {
	switch name {
	case "read":
		return "Read"
	case "write":
		return "Write"
	case "edit":
		return "Edit"
	case "bash":
		return "Bash"
	case "grep":
		return "Grep"
	case "find", "ls":
		return "Glob"
	case "fetch_content":
		return "Read"
	case "ast_grep_outline", "ast_grep_search", "ast_grep_scan":
		return "Grep"
	case "ast_grep_rewrite":
		return "Edit"
	default:
		return name
	}
}
