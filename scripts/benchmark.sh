#!/bin/bash
set -e

RTK="./target/release/rtk"
BENCH_DIR="scripts/benchmark"
REPORT="benchmark-report.md"

# Nettoyer et créer le dossier benchmark
rm -rf "$BENCH_DIR"
mkdir -p "$BENCH_DIR/unix"
mkdir -p "$BENCH_DIR/rtk"
mkdir -p "$BENCH_DIR/diff"

# Fonction pour compter les tokens (~4 chars = 1 token)
count_tokens() {
  local input="$1"
  local len=${#input}
  echo $(( (len + 3) / 4 ))
}

# Fonction pour créer un nom de fichier safe
safe_name() {
  echo "$1" | tr ' /' '_-' | tr -cd 'a-zA-Z0-9_-'
}

# Fonction de benchmark
bench() {
  local name="$1"
  local unix_cmd="$2"
  local rtk_cmd="$3"
  local filename=$(safe_name "$name")

  unix_out=$(eval "$unix_cmd" 2>/dev/null || true)
  rtk_out=$(eval "$rtk_cmd" 2>/dev/null || true)

  unix_tokens=$(count_tokens "$unix_out")
  rtk_tokens=$(count_tokens "$rtk_out")

  # Déterminer si RTK économise des tokens
  local use_rtk=true
  local status="✅"
  local prefix="GOOD"
  local recommended_cmd="$rtk_cmd"
  local recommended_out="$rtk_out"

  if [ "$rtk_tokens" -ge "$unix_tokens" ] && [ "$unix_tokens" -gt 0 ]; then
    use_rtk=false
    status="⚠️ SKIP"
    prefix="BAD"
    recommended_cmd="$unix_cmd"
    recommended_out="$unix_out"
  fi

  if [ "$unix_tokens" -gt 0 ]; then
    local diff_pct=$(( (unix_tokens - rtk_tokens) * 100 / unix_tokens ))
  else
    local diff_pct=0
  fi

  # Sauvegarder les outputs dans des fichiers md
  {
    echo "# Unix: $name"
    echo ""
    echo "\`\`\`bash"
    echo "$ $unix_cmd"
    echo "\`\`\`"
    echo ""
    echo "## Output"
    echo ""
    echo "\`\`\`"
    echo "$unix_out"
    echo "\`\`\`"
  } > "$BENCH_DIR/unix/${filename}.md"

  {
    echo "# RTK: $name"
    echo ""
    echo "\`\`\`bash"
    echo "$ $rtk_cmd"
    echo "\`\`\`"
    echo ""
    echo "## Output"
    echo ""
    echo "\`\`\`"
    echo "$rtk_out"
    echo "\`\`\`"
  } > "$BENCH_DIR/rtk/${filename}.md"

  # Générer le diff comparatif
  {
    echo "# Diff: $name"
    echo ""
    if [ "$use_rtk" = false ]; then
      echo "> ⚠️ **RTK adds tokens here!** Use Unix command instead."
      echo ""
    fi
    echo "| Metric | Unix | RTK | Saved | Status |"
    echo "|--------|------|-----|-------|--------|"
    echo "| Tokens | $unix_tokens | $rtk_tokens | $diff_pct% | $status |"
    echo "| Chars | ${#unix_out} | ${#rtk_out} | | |"
    echo ""
    echo "## Recommended Command"
    echo ""
    echo "\`\`\`bash"
    echo "$ $recommended_cmd"
    echo "\`\`\`"
    echo ""
    echo "## Commands"
    echo ""
    echo "\`\`\`bash"
    echo "# Unix"
    echo "$ $unix_cmd"
    echo ""
    echo "# RTK"
    echo "$ $rtk_cmd"
    echo "\`\`\`"
    echo ""
    echo "---"
    echo ""
    echo "## Unix Output"
    echo ""
    echo "\`\`\`"
    echo "$unix_out"
    echo "\`\`\`"
    echo ""
    echo "---"
    echo ""
    echo "## RTK Output"
    echo ""
    echo "\`\`\`"
    echo "$rtk_out"
    echo "\`\`\`"
    echo ""
    echo "---"
    echo ""
    echo "## Diff (Unix → RTK)"
    echo ""
    echo "\`\`\`diff"
    diff <(echo "$unix_out") <(echo "$rtk_out") || true
    echo "\`\`\`"
  } > "$BENCH_DIR/diff/${prefix}-${filename}.md"
  rtk_tokens=$(count_tokens "$rtk_out")

  if [ "$unix_tokens" -gt 0 ]; then
    saved=$((unix_tokens - rtk_tokens))
    pct=$((saved * 100 / unix_tokens))
  else
    saved=0
    pct=0
  fi

  # Accumuler pour le résumé (seulement si RTK économise)
  TOTAL_UNIX=$((TOTAL_UNIX + unix_tokens))
  if [ "$use_rtk" = true ]; then
    TOTAL_RTK=$((TOTAL_RTK + rtk_tokens))
  else
    TOTAL_RTK=$((TOTAL_RTK + unix_tokens))
    SKIPPED=$((SKIPPED + 1))
  fi

  echo "| $name | $unix_tokens | $rtk_tokens | $diff_pct% | $status |" >> "$REPORT"

  # Ajouter aux recommandations
  echo "| $name | \`$recommended_cmd\` |" >> "$RECOMMEND"
}

# Init totaux
TOTAL_UNIX=0
TOTAL_RTK=0
SKIPPED=0
RECOMMEND="$BENCH_DIR/recommendations.md"

# Header rapport
echo "# RTK Benchmark Report" > "$REPORT"
echo "" >> "$REPORT"
echo "| Command | Unix tokens | RTK tokens | Saved | Status |" >> "$REPORT"
echo "|---------|-------------|------------|-------|--------|" >> "$REPORT"

# Header recommandations
echo "# RTK Recommended Commands" > "$RECOMMEND"
echo "" >> "$RECOMMEND"
echo "Use these commands for optimal token savings:" >> "$RECOMMEND"
echo "" >> "$RECOMMEND"
echo "| Command | Recommended |" >> "$RECOMMEND"
echo "|---------|-------------|" >> "$RECOMMEND"

# ===================
# ls
# ===================
echo "" >> "$REPORT"
echo "| **ls** | | | |" >> "$REPORT"
bench "ls" "ls -la" "$RTK ls"
bench "ls src/" "ls -la src/" "$RTK ls src/"
bench "ls -a" "ls -la" "$RTK ls -a"
bench "ls -d 3" "find . -maxdepth 3 -type f" "$RTK ls -d 3"
bench "ls -d 3 -f tree" "tree -L 3 2>/dev/null || find . -maxdepth 3" "$RTK ls -d 3 -f tree"
bench "ls -f json" "ls -la" "$RTK ls -f json"
bench "ls -a -d 2 -f tree" "tree -L 2 -a 2>/dev/null || find . -maxdepth 2" "$RTK ls -a -d 2 -f tree"

# ===================
# read
# ===================
echo "" >> "$REPORT"
echo "| **read** | | | |" >> "$REPORT"
bench "read" "cat src/main.rs" "$RTK read src/main.rs"
bench "read -l minimal" "cat src/main.rs" "$RTK read src/main.rs -l minimal"
bench "read -l aggressive" "cat src/main.rs" "$RTK read src/main.rs -l aggressive"
bench "read -n" "cat -n src/main.rs" "$RTK read src/main.rs -n"


# ===================
# find
# ===================
echo "" >> "$REPORT"
echo "| **find** | | | |" >> "$REPORT"
bench "find *" "find . -type f" "$RTK find '*'"
bench "find *.rs" "find . -name '*.rs' -type f" "$RTK find '*.rs'"
bench "find *.toml" "find . -name '*.toml' -type f" "$RTK find '*.toml'"
bench "find --max 10" "find . -type f | head -10" "$RTK find '*' --max 10"
bench "find --max 100" "find . -type f | head -100" "$RTK find '*' --max 100"

# ===================
# diff
# ===================
echo "" >> "$REPORT"
echo "| **diff** | | | |" >> "$REPORT"
# Créer fichiers temp pour test diff
echo -e "line1\nline2\nline3" > /tmp/rtk_bench_f1.txt
echo -e "line1\nmodified\nline3\nline4" > /tmp/rtk_bench_f2.txt
bench "diff" "diff /tmp/rtk_bench_f1.txt /tmp/rtk_bench_f2.txt || true" "$RTK diff /tmp/rtk_bench_f1.txt /tmp/rtk_bench_f2.txt"
rm -f /tmp/rtk_bench_f1.txt /tmp/rtk_bench_f2.txt

# ===================
# git
# ===================
echo "" >> "$REPORT"
echo "| **git** | | | |" >> "$REPORT"
bench "git status" "git status" "$RTK git status"
bench "git log -n 10" "git log -10 --oneline" "$RTK git log -n 10"
bench "git log -n 5" "git log -5" "$RTK git log -n 5"
bench "git diff" "git diff HEAD~1 2>/dev/null || echo ''" "$RTK git diff"

# ===================
# grep
# ===================
echo "" >> "$REPORT"
echo "| **grep** | | | |" >> "$REPORT"
bench "grep fn" "grep -rn 'fn ' src/ || true" "$RTK grep 'fn ' src/"
bench "grep struct" "grep -rn 'struct ' src/ || true" "$RTK grep 'struct ' src/"
bench "grep -l 40" "grep -rn 'fn ' src/ || true" "$RTK grep 'fn ' src/ -l 40"
bench "grep --max 20" "grep -rn 'fn ' src/ | head -20 || true" "$RTK grep 'fn ' src/ --max 20"
bench "grep -c" "grep -ron 'fn ' src/ || true" "$RTK grep 'fn ' src/ -c"

# ===================
# json
# ===================
echo "" >> "$REPORT"
echo "| **json** | | | |" >> "$REPORT"
# Créer un fichier JSON de test
cat > /tmp/rtk_bench.json << 'JSONEOF'
{
  "name": "rtk",
  "version": "0.2.1",
  "config": {
    "debug": false,
    "max_depth": 10,
    "filters": ["node_modules", "target", ".git"]
  },
  "dependencies": {
    "serde": "1.0",
    "clap": "4.0",
    "anyhow": "1.0"
  }
}
JSONEOF
bench "json" "cat /tmp/rtk_bench.json" "$RTK json /tmp/rtk_bench.json"
bench "json -d 2" "cat /tmp/rtk_bench.json" "$RTK json /tmp/rtk_bench.json -d 2"
rm -f /tmp/rtk_bench.json

# ===================
# deps
# ===================
echo "" >> "$REPORT"
echo "| **deps** | | | |" >> "$REPORT"
bench "deps" "cat Cargo.toml" "$RTK deps"

# ===================
# env
# ===================
echo "" >> "$REPORT"
echo "| **env** | | | |" >> "$REPORT"
bench "env" "env" "$RTK env"
bench "env -f PATH" "env | grep PATH" "$RTK env -f PATH"
bench "env --show-all" "env" "$RTK env --show-all"

# ===================
# err
# ===================
echo "" >> "$REPORT"
echo "| **err** | | | |" >> "$REPORT"
bench "err echo test" "echo test 2>&1" "$RTK err echo test"

# ===================
# test
# ===================
echo "" >> "$REPORT"
echo "| **test** | | | |" >> "$REPORT"
bench "test cargo test" "cargo test 2>&1 || true" "$RTK test cargo test"

# ===================
# log
# ===================
echo "" >> "$REPORT"
echo "| **log** | | | |" >> "$REPORT"
# Créer un fichier log de test avec lignes répétées (pour montrer la déduplication)
LOG_FILE="$BENCH_DIR/sample.log"
cat > "$LOG_FILE" << 'LOGEOF'
2024-01-15 10:00:01 INFO  Application started
2024-01-15 10:00:02 INFO  Loading configuration
2024-01-15 10:00:03 ERROR Connection failed: timeout
2024-01-15 10:00:04 ERROR Connection failed: timeout
2024-01-15 10:00:05 ERROR Connection failed: timeout
2024-01-15 10:00:06 ERROR Connection failed: timeout
2024-01-15 10:00:07 ERROR Connection failed: timeout
2024-01-15 10:00:08 WARN  Retrying connection
2024-01-15 10:00:09 INFO  Connection established
2024-01-15 10:00:10 INFO  Processing request
2024-01-15 10:00:11 INFO  Processing request
2024-01-15 10:00:12 INFO  Processing request
2024-01-15 10:00:13 INFO  Request completed
LOGEOF
bench "log" "cat $LOG_FILE" "$RTK log $LOG_FILE"

# ===================
# summary
# ===================
echo "" >> "$REPORT"
echo "| **summary** | | | |" >> "$REPORT"
bench "summary cargo --help" "cargo --help" "$RTK summary cargo --help"
bench "summary rustc --help" "rustc --help 2>/dev/null || echo 'rustc not found'" "$RTK summary rustc --help"

# ===================
# Modern JavaScript Stack (skip si pas de package.json)
# ===================
if [ -f "package.json" ]; then
  echo "" >> "$REPORT"
  echo "| **Modern JS Stack** | | | |" >> "$REPORT"

  # TypeScript compiler
  if command -v tsc &> /dev/null || [ -f "node_modules/.bin/tsc" ]; then
    bench "tsc" "tsc --noEmit 2>&1 || true" "$RTK tsc --noEmit"
  fi

  # Prettier format checker
  if command -v prettier &> /dev/null || [ -f "node_modules/.bin/prettier" ]; then
    bench "prettier --check" "prettier --check . 2>&1 || true" "$RTK prettier --check ."
  fi

  # ESLint/Biome linter
  if command -v eslint &> /dev/null || [ -f "node_modules/.bin/eslint" ]; then
    bench "lint" "eslint . 2>&1 || true" "$RTK lint ."
  fi

  # Next.js build (if Next.js project)
  if [ -f "next.config.js" ] || [ -f "next.config.mjs" ] || [ -f "next.config.ts" ]; then
    if command -v next &> /dev/null || [ -f "node_modules/.bin/next" ]; then
      bench "next build" "next build 2>&1 || true" "$RTK next build"
    fi
  fi

  # Playwright E2E tests (if Playwright configured)
  if [ -f "playwright.config.ts" ] || [ -f "playwright.config.js" ]; then
    if command -v playwright &> /dev/null || [ -f "node_modules/.bin/playwright" ]; then
      bench "playwright test" "playwright test 2>&1 || true" "$RTK playwright test"
    fi
  fi

  # Prisma (if Prisma schema exists)
  if [ -f "prisma/schema.prisma" ]; then
    if command -v prisma &> /dev/null || [ -f "node_modules/.bin/prisma" ]; then
      bench "prisma generate" "prisma generate 2>&1 || true" "$RTK prisma generate"
    fi
  fi
fi

# ===================
# docker (skip si pas dispo)
# ===================
if command -v docker &> /dev/null; then
  echo "" >> "$REPORT"
  echo "| **docker** | | | |" >> "$REPORT"
  bench "docker ps" "docker ps 2>/dev/null || true" "$RTK docker ps"
  bench "docker images" "docker images 2>/dev/null || true" "$RTK docker images"
fi

# ===================
# kubectl (skip si pas dispo)
# ===================
if command -v kubectl &> /dev/null; then
  echo "" >> "$REPORT"
  echo "| **kubectl** | | | |" >> "$REPORT"
  bench "kubectl pods" "kubectl get pods 2>/dev/null || true" "$RTK kubectl pods"
  bench "kubectl services" "kubectl get services 2>/dev/null || true" "$RTK kubectl services"
fi

# ===================
# Résumé global
# ===================
echo "" >> "$REPORT"
echo "## Summary" >> "$REPORT"
echo "" >> "$REPORT"

if [ "$TOTAL_UNIX" -gt 0 ]; then
  TOTAL_SAVED=$((TOTAL_UNIX - TOTAL_RTK))
  TOTAL_PCT=$((TOTAL_SAVED * 100 / TOTAL_UNIX))
  echo "| Metric | Value |" >> "$REPORT"
  echo "|--------|-------|" >> "$REPORT"
  echo "| Total Unix tokens | $TOTAL_UNIX |" >> "$REPORT"
  echo "| Total RTK tokens | $TOTAL_RTK |" >> "$REPORT"
  echo "| Total saved | $TOTAL_SAVED |" >> "$REPORT"
  echo "| **Global savings** | **$TOTAL_PCT%** |" >> "$REPORT"
  echo "| Commands skipped (no gain) | $SKIPPED |" >> "$REPORT"
fi

echo "" >> "$REPORT"
echo "---" >> "$REPORT"
echo "Generated on $(date -u +"%Y-%m-%d %H:%M:%S UTC")" >> "$REPORT"

echo ""
echo "=== BENCHMARK REPORT ==="
cat "$REPORT"

echo ""
echo "=== FILES GENERATED ==="
echo "Unix outputs: $BENCH_DIR/unix/"
echo "RTK outputs:  $BENCH_DIR/rtk/"
echo "Diff files:   $BENCH_DIR/diff/"
ls -1 "$BENCH_DIR/diff/" | wc -l | xargs echo "Total files:"
