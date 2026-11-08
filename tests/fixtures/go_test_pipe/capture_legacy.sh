#!/usr/bin/env bash
# Re-capture the historical "bench" action with Go 1.24.6 (b.Log -> --- BENCH).
set -euo pipefail
out=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
export GOPATH="$work/gopath" GOCACHE="$work/go-build-cache" GOTOOLCHAIN=local GOFLAGS=-modcacherw
printf 'module example.com/rtk-pipe-fixture\n\ngo 1.24.0\n' > go.mod
mkdir cases empty
printf 'package empty\n' > empty/empty.go
cat > cases/capture_test.go <<'EOF'
package cases

import "testing"

func TestPass(t *testing.T) { t.Log("captured pass") }
func TestParallelA(t *testing.T) { t.Parallel(); t.Log("parallel A") }
func TestParallelB(t *testing.T) { t.Parallel(); t.Log("parallel B") }
func TestSkip(t *testing.T) { t.Skip("captured skip") }
func BenchmarkLogged(b *testing.B) {
    b.Log("captured benchmark log")
    for i := 0; i < b.N; i++ { _ = i }
}
EOF
go test -json -count=1 -bench BenchmarkLogged -benchtime=1x ./... > "$out/legacy_core.jsonl" 2>&1
go test -json -v=false -count=1 -run '^$' -bench BenchmarkLogged -benchtime=1x ./cases > "$out/legacy_bench.jsonl" 2>&1
go version
