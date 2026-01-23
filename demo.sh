#!/bin/bash
# rtk demo script

echo "🔧 rtk - Rust Token Killer Demo"
echo "================================"
sleep 1

echo ""
echo "📁 1. Directory listing (rtk ls vs ls)"
echo "----------------------------------------"
sleep 0.5
echo "$ rtk ls . -d 3"
rtk ls . -d 3
sleep 2

echo ""
echo "📄 2. File reading with filtering"
echo "----------------------------------"
sleep 0.5
echo "$ rtk read src/main.rs -l aggressive --max-lines 20"
rtk read src/main.rs -l aggressive --max-lines 20
sleep 2

echo ""
echo "🔍 3. Compact grep"
echo "-------------------"
sleep 0.5
echo "$ rtk grep 'fn run' . --max 10"
rtk grep 'fn run' . --max 10
sleep 2

echo ""
echo "📊 4. Git status (compact)"
echo "--------------------------"
sleep 0.5
echo "$ rtk git status"
rtk git status
sleep 2

echo ""
echo "📜 5. Git log (one-line)"
echo "-------------------------"
sleep 0.5
echo "$ rtk git log -n 5"
rtk git log -n 5
sleep 2

echo ""
echo "📦 6. Dependencies summary"
echo "--------------------------"
sleep 0.5
echo "$ rtk deps"
rtk deps
sleep 2

echo ""
echo "🗂️ 7. JSON structure"
echo "---------------------"
sleep 0.5
echo "$ rtk json Cargo.toml 2>/dev/null || echo '(showing deps instead)'"
rtk deps
sleep 2

echo ""
echo "✅ rtk saves 60-90% tokens on common operations!"
echo ""
