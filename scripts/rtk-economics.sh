#!/bin/bash
# rtk-economics.sh
# Combine ccusage (tokens spent) with rtk (tokens saved) for economic analysis

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Get current month
CURRENT_MONTH=$(date +%Y-%m)

echo -e "${BLUE}📊 RTK Economic Impact Analysis${NC}"
echo "════════════════════════════════════════════════════════════════"
echo

# Check if ccusage is available
if ! command -v ccusage &> /dev/null; then
    echo -e "${RED}Error: ccusage not found${NC}"
    echo "Install: npm install -g @anthropics/claude-code-usage"
    exit 1
fi

# Check if rtk is available
if ! command -v rtk &> /dev/null; then
    echo -e "${RED}Error: rtk not found${NC}"
    echo "Install: cargo install --path ."
    exit 1
fi

# Fetch ccusage data
echo -e "${YELLOW}Fetching token usage data from ccusage...${NC}"
if ! ccusage_json=$(ccusage monthly --json 2>/dev/null); then
    echo -e "${RED}Failed to fetch ccusage data${NC}"
    exit 1
fi

# Fetch rtk data
echo -e "${YELLOW}Fetching token savings data from rtk...${NC}"
if ! rtk_json=$(rtk gain --monthly --format json 2>/dev/null); then
    echo -e "${RED}Failed to fetch rtk data${NC}"
    exit 1
fi

echo

# Parse ccusage data for current month
ccusage_cost=$(echo "$ccusage_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .totalCost // 0")
ccusage_input=$(echo "$ccusage_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .inputTokens // 0")
ccusage_output=$(echo "$ccusage_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .outputTokens // 0")
ccusage_total=$(echo "$ccusage_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .totalTokens // 0")

# Parse rtk data for current month
rtk_saved=$(echo "$rtk_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .saved_tokens // 0")
rtk_commands=$(echo "$rtk_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .commands // 0")
rtk_input=$(echo "$rtk_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .input_tokens // 0")
rtk_output=$(echo "$rtk_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .output_tokens // 0")
rtk_pct=$(echo "$rtk_json" | jq -r ".monthly[] | select(.month == \"$CURRENT_MONTH\") | .savings_pct // 0")

# Estimate cost avoided (rough: $0.0001/token for mixed usage)
# More accurate would be to use ccusage's model-specific pricing
saved_cost=$(echo "scale=2; $rtk_saved * 0.0001" | bc 2>/dev/null || echo "0")

# Calculate total without rtk
total_without_rtk=$(echo "scale=2; $ccusage_cost + $saved_cost" | bc 2>/dev/null || echo "$ccusage_cost")

# Calculate savings percentage
if (( $(echo "$total_without_rtk > 0" | bc -l) )); then
    savings_pct=$(echo "scale=1; ($saved_cost / $total_without_rtk) * 100" | bc 2>/dev/null || echo "0")
else
    savings_pct="0"
fi

# Calculate cost per command
if [ "$rtk_commands" -gt 0 ]; then
    cost_per_cmd_with=$(echo "scale=2; $ccusage_cost / $rtk_commands" | bc 2>/dev/null || echo "0")
    cost_per_cmd_without=$(echo "scale=2; $total_without_rtk / $rtk_commands" | bc 2>/dev/null || echo "0")
else
    cost_per_cmd_with="N/A"
    cost_per_cmd_without="N/A"
fi

# Format numbers
format_number() {
    local num=$1
    if [ "$num" = "0" ] || [ "$num" = "N/A" ]; then
        echo "$num"
    else
        echo "$num" | numfmt --to=si 2>/dev/null || echo "$num"
    fi
}

# Display report
cat << EOF
${GREEN}💰 Economic Impact Report - $CURRENT_MONTH${NC}
════════════════════════════════════════════════════════════════

${BLUE}Tokens Consumed (via Claude API):${NC}
  Input tokens:        $(format_number $ccusage_input)
  Output tokens:       $(format_number $ccusage_output)
  Total tokens:        $(format_number $ccusage_total)
  ${RED}Actual cost:         \$$ccusage_cost${NC}

${BLUE}Tokens Saved by rtk:${NC}
  Commands executed:   $rtk_commands
  Input avoided:       $(format_number $rtk_input) tokens
  Output generated:    $(format_number $rtk_output) tokens
  Total saved:         $(format_number $rtk_saved) tokens (${rtk_pct}% reduction)
  ${GREEN}Cost avoided:        ~\$$saved_cost${NC}

${BLUE}Economic Analysis:${NC}
  Cost without rtk:    \$$total_without_rtk (estimated)
  Cost with rtk:       \$$ccusage_cost (actual)
  ${GREEN}Net savings:         \$$saved_cost ($savings_pct%)${NC}
  ROI:                 ${GREEN}Infinite${NC} (rtk is free)

${BLUE}Efficiency Metrics:${NC}
  Cost per command:    \$$cost_per_cmd_without → \$$cost_per_cmd_with
  Tokens per command:  $(echo "scale=0; $rtk_input / $rtk_commands" | bc 2>/dev/null || echo "N/A") → $(echo "scale=0; $rtk_output / $rtk_commands" | bc 2>/dev/null || echo "N/A")

${BLUE}12-Month Projection:${NC}
  Annual savings:      ~\$$(echo "scale=2; $saved_cost * 12" | bc 2>/dev/null || echo "0")
  Commands needed:     $(echo "$rtk_commands * 12" | bc 2>/dev/null || echo "0") (at current rate)

════════════════════════════════════════════════════════════════

${YELLOW}Note:${NC} Cost estimates use \$0.0001/token average. Actual pricing varies by model.
See ccusage for precise model-specific costs.

${GREEN}Recommendation:${NC} Focus rtk usage on high-frequency commands (git, grep, ls)
for maximum cost reduction.

EOF
