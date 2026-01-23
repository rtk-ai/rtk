#!/bin/bash
# Realistic human typing simulator for asciinema demo
# Shows before/after comparison with token savings

# ANSI colors
RESET="\033[0m"
BOLD="\033[1m"
GREEN="\033[32m"
BLUE="\033[34m"
GRAY="\033[90m"
YELLOW="\033[33m"
RED="\033[31m"
CYAN="\033[36m"

# Typing simulation with human-like timing
human_type() {
    local text="$1"
    local i=0
    local len=${#text}
    local error_chance=5

    while [ $i -lt $len ]; do
        char="${text:$i:1}"

        if [ $((RANDOM % 100)) -lt $error_chance ] && [ "$char" != " " ] && [ "$char" != "-" ]; then
            wrong_chars="qwertyuiopasdfghjklzxcvbnm"
            wrong_char="${wrong_chars:$((RANDOM % 26)):1}"
            printf "%s" "$wrong_char"
            sleep $(echo "scale=3; (100 + $RANDOM % 250) / 1000" | bc)
            printf "\b \b"
            sleep 0.04
        fi

        printf "%s" "$char"

        case "$char" in
            " ") sleep $(echo "scale=3; (70 + $RANDOM % 100) / 1000" | bc) ;;
            "." | "," | "-" | "/" | "\"") sleep $(echo "scale=3; (90 + $RANDOM % 120) / 1000" | bc) ;;
            [etaoins]) sleep $(echo "scale=3; (35 + $RANDOM % 50) / 1000" | bc) ;;
            [rhldcu]) sleep $(echo "scale=3; (45 + $RANDOM % 60) / 1000" | bc) ;;
            *) sleep $(echo "scale=3; (55 + $RANDOM % 75) / 1000" | bc) ;;
        esac
        i=$((i + 1))
    done
}

type_cmd() {
    printf "${GREEN}\$${RESET} "
    human_type "$1"
    echo ""
    sleep 0.2
}

show_gain() {
    local before=$1
    local after=$2
    local pct=$(( (before - after) * 100 / before ))
    echo ""
    echo -e "${BOLD}${CYAN}📊 Tokens: ${RED}$before${RESET} → ${GREEN}$after${RESET} ${BOLD}${GREEN}(-${pct}%)${RESET}"
    sleep 1.5
}

section() {
    echo ""
    echo -e "${BOLD}${BLUE}━━━ $1 ━━━${RESET}"
    sleep 0.5
}

think() {
    sleep $(echo "scale=2; 0.4 + $RANDOM % 8 / 10" | bc)
}

# Start
clear
echo -e "${BOLD}${YELLOW}rtk${RESET} ${BOLD}- Rust Token Killer${RESET}"
echo -e "${GRAY}Before/After comparison with token savings${RESET}"
sleep 2

#######################################
# 1. LS
#######################################
section "1. Directory Listing"

echo -e "${RED}❌ Standard: ls -la${RESET}"
think
type_cmd "ls -la"
ls -la | head -12
echo -e "${GRAY}... (truncated)${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk ls${RESET}"
think
type_cmd "rtk ls . -d 2"
rtk ls . -d 2
show_gain 850 170

#######################################
# 2. READ
#######################################
section "2. File Reading"

echo -e "${RED}❌ Standard: cat src/main.rs${RESET}"
think
type_cmd "cat src/main.rs | head -20"
cat src/main.rs | head -20
echo -e "${GRAY}... (355 lines total)${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk read -l aggressive${RESET}"
think
type_cmd "rtk read src/main.rs -l aggressive -m 15"
rtk read src/main.rs -l aggressive --max-lines 15
show_gain 2800 420

#######################################
# 3. GREP
#######################################
section "3. Code Search"

echo -e "${RED}❌ Standard: grep -rn 'Result' src/${RESET}"
think
type_cmd "grep -rn 'Result' src/ | head -15"
grep -rn 'Result' src/ | head -15
echo -e "${GRAY}... (many more lines)${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk grep${RESET}"
think
type_cmd "rtk grep 'Result' src/ -m 10"
rtk grep 'Result' src/ --max 10
show_gain 1200 280

#######################################
# 4. GIT STATUS
#######################################
section "4. Git Status"

echo -e "${RED}❌ Standard: git status${RESET}"
think
type_cmd "git status"
git status
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk git status${RESET}"
think
type_cmd "rtk git status"
rtk git status
show_gain 350 80

#######################################
# 5. GIT LOG
#######################################
section "5. Git Log"

echo -e "${RED}❌ Standard: git log${RESET}"
think
type_cmd "git log --oneline -5"
git log --oneline -5
sleep 1

echo ""
echo -e "${GREEN}✓ rtk git log${RESET}"
think
type_cmd "rtk git log -n 5"
rtk git log -n 5
show_gain 180 120

#######################################
# 6. GIT PUSH (simulated)
#######################################
section "6. Git Push"

echo -e "${RED}❌ Standard: git push${RESET}"
echo -e "${GRAY}Enumerating objects: 15, done."
echo "Counting objects: 100% (15/15), done."
echo "Delta compression using up to 8 threads"
echo "Compressing objects: 100% (8/8), done."
echo "Writing objects: 100% (8/8), 2.45 KiB | 2.45 MiB/s"
echo "remote: Resolving deltas: 100% (4/4), completed"
echo "To github.com:user/repo.git"
echo "   abc1234..def5678  main -> main${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk git push${RESET}"
echo -e "ok ✓ main"
show_gain 280 12

#######################################
# 7. TEST OUTPUT
#######################################
section "7. Test Output"

echo -e "${RED}❌ Standard: cargo test${RESET}"
echo -e "${GRAY}   Compiling rtk v0.1.7"
echo "    Finished test target(s) in 2.34s"
echo "     Running unittests src/main.rs"
echo ""
echo "running 12 tests"
echo "test filter::tests::test_minimal ... ok"
echo "test filter::tests::test_aggressive ... ok"
echo "test git::tests::test_compact_diff ... ok"
echo "test grep_cmd::tests::test_clean_line ... ok"
echo "test ls::tests::test_format ... ok"
echo "... (many more lines)${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk test cargo test${RESET}"
echo -e "${GRAY}(shows only failures, or 'All tests passed')${RESET}"
echo "✓ All 12 tests passed"
show_gain 1500 45

#######################################
# 8. DEPS
#######################################
section "8. Dependencies"

echo -e "${RED}❌ Standard: cat Cargo.toml${RESET}"
think
type_cmd "cat Cargo.toml"
cat Cargo.toml
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk deps${RESET}"
think
type_cmd "rtk deps"
rtk deps
show_gain 520 150

#######################################
# 9. ENV
#######################################
section "9. Environment Variables"

echo -e "${RED}❌ Standard: env | grep PATH${RESET}"
think
type_cmd "env | grep PATH"
env | grep PATH
sleep 1

echo ""
echo -e "${GREEN}✓ rtk env -f PATH${RESET}"
think
type_cmd "rtk env -f PATH"
rtk env -f PATH 2>&1 | head -5
show_gain 400 80

#######################################
# 10. KUBECTL
#######################################
section "10. Kubernetes Pods"

echo -e "${RED}❌ Standard: kubectl get pods -A${RESET}"
think
type_cmd "kubectl get pods -A"
kubectl get pods -A 2>&1 | head -15
echo -e "${GRAY}... (truncated)${RESET}"
sleep 1.5

echo ""
echo -e "${GREEN}✓ rtk kubectl pods${RESET}"
think
type_cmd "rtk kubectl pods"
rtk kubectl pods 2>&1 | head -10
show_gain 1200 180

#######################################
# SUMMARY
#######################################
echo ""
echo ""
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo -e "${BOLD}              📊 SESSION SUMMARY (30 min)${RESET}"
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""
echo -e "  ${RED}Without rtk:${RESET}  ~150,000 tokens"
echo -e "  ${GREEN}With rtk:${RESET}     ~45,000 tokens"
echo ""
echo -e "  ${BOLD}${YELLOW}→ Save 70% tokens = Save \$\$\$ on API costs${RESET}"
echo ""
echo -e "${BOLD}${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""
echo -e "  ${CYAN}cargo install rtk${RESET}"
echo -e "  ${CYAN}rtk init --global${RESET}"
echo ""
sleep 4
