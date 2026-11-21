Captured with Go 1.26.10 using `go test -json -p 1 -count=1 ./...`
in the module `example.com/stream`. The command exits 1.

`ok/ok_test.go`:

```go
package ok

import "testing"

func TestPass(t *testing.T) {}
func TestSkip(t *testing.T) { t.Skip("not available") }
```

`zz/z_test.go`:

```go
package zz

import "testing"

func TestX(t *testing.T) {
    t.Fatal("expected 1, got 2")
}
```
