#!/usr/bin/env bash
# Capture one benchmark run twice, chatty and quiet (-v=false), so the two
# streams of the same run can be compared. Covers sub-benchmarks, a skipped
# benchmark and a benchmark that writes to stdout while it runs. A third run
# combines -fuzz with -bench (fuzzing takes a single package).
set -euo pipefail
out=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
export GOPATH="$work/gopath" GOCACHE="$work/go-build-cache" GOTOOLCHAIN=local GOFLAGS=-modcacherw
printf 'module example.com/rtk-pipe-fixture\n\ngo 1.24.0\n' > go.mod
mkdir bench
cat > bench/bench_test.go <<'EOF'
package bench

import (
	"fmt"
	"testing"
)

func TestPass(t *testing.T) {}

func BenchmarkX(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = i
	}
}

func BenchmarkSub(b *testing.B) {
	for _, n := range []int{0, 1} {
		b.Run(fmt.Sprintf("n=%d", n), func(b *testing.B) {
			for i := 0; i < b.N; i++ {
				_ = i + n
			}
		})
	}
}

func BenchmarkSkip(b *testing.B) { b.Skip("captured benchmark skip") }

func BenchmarkPrint(b *testing.B) {
	fmt.Println("warming cache")
	for i := 0; i < b.N; i++ {
		_ = i
	}
}
EOF
mkdir fz
cat > fz/fz_test.go <<'EOF'
package fz

import "testing"

func FuzzEcho(f *testing.F) {
	f.Add("seed")
	f.Fuzz(func(t *testing.T, s string) { _ = s })
}

func BenchmarkEcho(b *testing.B) {
	for i := 0; i < b.N; i++ {
		_ = i
	}
}
EOF
go test -json -count=1 -run '^TestPass$' -bench . -benchtime=2x ./bench > "$out/bench_chatty.jsonl" 2>&1
go test -json -v=false -count=1 -run '^TestPass$' -bench . -benchtime=2x ./bench > "$out/bench_quiet.jsonl" 2>&1
go test -json -count=1 -run '^$' -bench . -benchtime=2x -fuzz FuzzEcho -fuzztime=20x ./fz > "$out/bench_fuzz.jsonl" 2>&1
go version
