#!/usr/bin/env bash
# Re-capture producer output with Go >= 1.26. Not run by Cargo/CI.
set -euo pipefail
out=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
export GOPATH="$work/gopath" GOCACHE="$work/go-build-cache" GOTOOLCHAIN=local GOFLAGS=-modcacherw
cat > go.mod <<'EOF'
module example.com/rtk-pipe-fixture

go 1.26.0

require (
    golang.org/x/sync v0.10.0
    golang.org/x/text v0.21.0
)
EOF
mkdir cases empty broken
cat > empty/empty.go <<'EOF'
package empty
EOF
cat > broken/broken.go <<'EOF'
package broken

var _ = missing
EOF
cat > cases/capture_test.go <<'EOF'
package cases

import (
    "os"
    "path/filepath"
    "testing"

    "golang.org/x/sync/errgroup"
    "golang.org/x/text/cases"
    "golang.org/x/text/language"
)

func TestPass(t *testing.T) {
    var group errgroup.Group
    group.Go(func() error { return nil })
    if err := group.Wait(); err != nil { t.Fatal(err) }
    if cases.Title(language.English).String("hello") != "Hello" { t.Fatal("title mismatch") }
}
func TestParallelA(t *testing.T) { t.Parallel(); t.Log("parallel A") }
func TestParallelB(t *testing.T) { t.Parallel(); t.Log("parallel B") }
func TestSkip(t *testing.T) { t.Skip("captured skip") }
func TestAttr(t *testing.T) { t.Attr("issue", "4455") }
func TestArtifact(t *testing.T) {
    if err := os.WriteFile(filepath.Join(t.ArtifactDir(), "result.txt"), []byte("captured artifact\n"), 0600); err != nil { t.Fatal(err) }
}
func TestFail(t *testing.T) { t.Error("expected 1, got 2") }
func BenchmarkLogged(b *testing.B) {
    b.Log("captured benchmark log")
    for i := 0; i < b.N; i++ { _ = i }
}
EOF
go mod tidy
# These captures deliberately retain timestamps, elapsed times and output bytes.
go test -json -count=1 -run 'Test(Pass|ParallelA|ParallelB|Skip)$' -bench BenchmarkLogged -benchtime=1x ./cases ./empty > "$out/core.jsonl" 2>&1
mkdir artifacts
go test -json -count=1 -artifacts -outputdir "$work/artifacts" -run 'Test(Attr|Artifact)$' ./cases ./empty > "$out/auxiliary.jsonl" 2>&1
set +e
go test -json -count=1 -run 'Test(Pass|Fail)$' ./cases > "$out/failure.jsonl" 2>&1
status=$?
set -e
test "$status" = 1
go test -c -o "$work/cases.test" ./cases
set +e
go tool test2json -t "$work/cases.test" -test.v=test2json -test.run 'Test(Pass|ParallelA|ParallelB|Skip|Fail)$' > "$out/package_less.jsonl" 2>&1
status=$?
set -e
test "$status" = 1
set +e
go tool test2json "$work/cases.test" -test.v=test2json -test.run 'Test(Pass|ParallelA|ParallelB|Skip|Fail)$' > "$out/package_less_no_time.jsonl" 2>&1
status=$?
set -e
test "$status" = 1
GOMODCACHE="$work/cold-cache" go test -json -count=1 -run 'Test(Pass|ParallelA|ParallelB|Skip)$' ./cases ./empty > "$out/mixed_downloads.jsonl" 2>&1
set +e
go build -json ./broken > "$out/build_only.jsonl" 2>&1
status=$?
set -e
test "$status" = 1
go version
