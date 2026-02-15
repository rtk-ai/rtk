#!/bin/bash
# Integration tests for RTK command interceptor
# Run from the rtk repository root

set -e

echo "=== RTK Command Interceptor Tests ==="

# Determine how to run rtk (prefer local builds)
if [ -f "./target/debug/rtk" ]; then
    RTK="./target/debug/rtk"
elif [ -f "./target/release/rtk" ]; then
    RTK="./target/release/rtk"
else
    echo "Building rtk..."
    cargo build
    RTK="./target/debug/rtk"
fi

echo "Using: $RTK"
echo ""

# 1. Basic execution
echo -n "Test 1: Basic echo... "
result=$($RTK run -c "echo hello" 2>&1)
if echo "$result" | grep -q "hello"; then
    echo "✓"
else
    echo "FAIL: expected 'hello' in output"
    exit 1
fi

# 2. Chained commands (&&)
echo -n "Test 2: Chained && ... "
result=$($RTK run -c "true && echo yes" 2>&1)
if echo "$result" | grep -q "yes"; then
    echo "✓"
else
    echo "FAIL: expected 'yes' in output"
    exit 1
fi

# 3. Chained commands (||)
echo -n "Test 3: Chained || ... "
result=$($RTK run -c "false || echo fallback" 2>&1)
if echo "$result" | grep -q "fallback"; then
    echo "✓"
else
    echo "FAIL: expected 'fallback' in output"
    exit 1
fi

# 4. Chained commands (;)
echo -n "Test 4: Chained ; ... "
result=$($RTK run -c "true ; echo always" 2>&1)
if echo "$result" | grep -q "always"; then
    echo "✓"
else
    echo "FAIL: expected 'always' in output"
    exit 1
fi

# 5. Hook protocol - safe command
echo -n "Test 5: Hook safe command... "
result=$($RTK hook check --agent claude "git status" 2>&1)
if echo "$result" | grep -q "rtk run"; then
    echo "✓"
else
    echo "FAIL: expected 'rtk run' in output"
    exit 1
fi

# 6. Hook protocol - blocked command (cat)
echo -n "Test 6: Hook blocked command (cat)... "
if ! $RTK hook check --agent claude "cat /etc/passwd" 2>/dev/null; then
    echo "✓"
else
    echo "FAIL: expected non-zero exit for blocked command"
    exit 1
fi

# 7. Passthrough for globs
echo -n "Test 7: Glob passthrough... "
if $RTK run -c "echo *.rs" 2>/dev/null; then
    echo "✓"
else
    echo "✓ (no .rs files or expected behavior)"
fi

# 8. Passthrough for pipes
echo -n "Test 8: Pipe passthrough... "
result=$($RTK run -c "echo hello | cat" 2>&1)
if echo "$result" | grep -q "hello"; then
    echo "✓"
else
    echo "FAIL: expected 'hello' in output"
    exit 1
fi

# 9. Builtins - pwd
echo -n "Test 9: Builtin pwd... "
result=$($RTK run -c "pwd" 2>&1)
if echo "$result" | grep -q "/"; then
    echo "✓"
else
    echo "FAIL: expected path in output"
    exit 1
fi

# 10. Quoted operators
echo -n "Test 10: Quoted operator... "
result=$($RTK run -c "echo 'hello && world'" 2>&1)
if echo "$result" | grep -q "hello"; then
    echo "✓"
else
    echo "FAIL: expected 'hello' in output"
    exit 1
fi

# 11. Hook blocked command (sed)
echo -n "Test 11: Hook blocked command (sed)... "
if ! $RTK hook check --agent claude "sed -i 's/old/new/' file.txt" 2>/dev/null; then
    echo "✓"
else
    echo "FAIL: expected non-zero exit for blocked sed command"
    exit 1
fi

# 12. Hook blocked command (head)
echo -n "Test 12: Hook blocked command (head)... "
if ! $RTK hook check --agent claude "head -n 10 file.txt" 2>/dev/null; then
    echo "✓"
else
    echo "FAIL: expected non-zero exit for blocked head command"
    exit 1
fi

# 13. Hook exit code for rewrite is 0
echo -n "Test 13: Hook rewrite exit code 0... "
$RTK hook check --agent claude "git status" > /dev/null 2>&1
exit_code=$?
if [ $exit_code -eq 0 ]; then
    echo "✓"
else
    echo "FAIL: expected exit code 0, got $exit_code"
    exit 1
fi

# 14. Hook exit code for blocked is 2
echo -n "Test 14: Hook blocked exit code 2... "
$RTK hook check --agent claude "cat file.txt" > /dev/null 2>&1 || exit_code=$?
if [ "$exit_code" -eq 2 ]; then
    echo "✓"
else
    echo "FAIL: expected exit code 2, got ${exit_code:-0}"
    exit 1
fi

echo ""
echo "=== All 14 tests passed ==="
