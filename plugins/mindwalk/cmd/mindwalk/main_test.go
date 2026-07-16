package main

import "testing"

func TestParseOutputArgsAllowsTrailingOutput(t *testing.T) {
	positional, out, err := parseOutputArgs([]string{"repo", "-o", "citymap.json"})
	if err != nil {
		t.Fatal(err)
	}
	if len(positional) != 1 || positional[0] != "repo" {
		t.Fatalf("positional = %#v", positional)
	}
	if out != "citymap.json" {
		t.Fatalf("out = %q", out)
	}
}
