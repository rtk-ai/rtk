#!/bin/bash
# ═══════════════════════════════════════════════════════════════
# RTK SECURITY HARDENING SCRIPT
# ═══════════════════════════════════════════════════════════════
# Version: 1.0.0
# Description: Comprehensive security hardening for RTK CLI tool
# ═══════════════════════════════════════════════════════════════

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
RTK_CONFIG_DIR="$HOME/.config/rtk"
RTK_DATA_DIR="$HOME/.local/share/rtk"
CLAUDE_CONFIG_DIR="$HOME/.claude"
ZSHRC="$HOME/.zshrc"
BACKUP_DIR="$HOME/.rtk-backup-$(date +%Y%m%d-%H%M%S)"

# ═══════════════════════════════════════════════════════════════
# HELPER FUNCTIONS
# ═══════════════════════════════════════════════════════════════

print_header() {
    echo -e "\n${BLUE}╔════════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║${NC}  $1"
    echo -e "${BLUE}╚════════════════════════════════════════════════════════════════╝${NC}\n"
}

print_step() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC}  $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_info() {
    echo -e "${BLUE}ℹ${NC}  $1"
}

backup_file() {
    local file=$1
    if [[ -f "$file" ]]; then
        mkdir -p "$BACKUP_DIR"
        cp "$file" "$BACKUP_DIR/"
        print_info "Backed up: $file → $BACKUP_DIR/"
    fi
}

# ═══════════════════════════════════════════════════════════════
# SECURITY AUDIT
# ═══════════════════════════════════════════════════════════════

audit_current_state() {
    print_header "🔍 SECURITY AUDIT - Current State"

    echo -e "${BLUE}1. RTK Installation:${NC}"
    if command -v rtk &> /dev/null; then
        print_step "RTK installed: $(rtk --version 2>/dev/null || echo 'Unknown version')"
    else
        print_error "RTK not installed"
        return 1
    fi

    echo -e "\n${BLUE}2. Telemetry Status:${NC}"
    if [[ -f "$RTK_CONFIG_DIR/config.toml" ]]; then
        if grep -q "enabled = false" "$RTK_CONFIG_DIR/config.toml" 2>/dev/null; then
            print_step "Telemetry disabled in config.toml"
        else
            print_warning "Telemetry may be enabled in config.toml"
        fi
    else
        print_warning "No config.toml found (using defaults)"
    fi

    if [[ "${RTK_TELEMETRY_DISABLED:-}" == "1" ]]; then
        print_step "Environment variable RTK_TELEMETRY_DISABLED=1"
    else
        print_warning "Environment variable RTK_TELEMETRY_DISABLED not set"
    fi

    echo -e "\n${BLUE}3. Tracking Status:${NC}"
    if [[ "${RTK_DB_PATH:-}" == "/dev/null" ]]; then
        print_step "Database redirected to /dev/null (no persistence)"
    else
        print_warning "Database path not set (using default)"
    fi

    if [[ -f "$RTK_DATA_DIR/history.db" ]]; then
        local size=$(du -h "$RTK_DATA_DIR/history.db" 2>/dev/null | cut -f1)
        print_warning "History database exists: $RTK_DATA_DIR/history.db ($size)"
    else
        print_step "No history database found"
    fi

    echo -e "\n${BLUE}4. Sensitive Files:${NC}"
    if [[ -f "$RTK_DATA_DIR/.device_salt" ]]; then
        print_warning "Device fingerprint salt exists: $RTK_DATA_DIR/.device_salt"
    else
        print_step "No device fingerprint salt found"
    fi

    if [[ -f "$RTK_DATA_DIR/.telemetry_last_ping" ]]; then
        print_warning "Telemetry ping marker exists: $RTK_DATA_DIR/.telemetry_last_ping"
    else
        print_step "No telemetry ping marker found"
    fi

    echo -e "\n${BLUE}5. Claude Code Integration:${NC}"
    if [[ -f "$CLAUDE_CONFIG_DIR/settings.json" ]]; then
        print_step "Claude Code settings found"
        if grep -q "deny" "$CLAUDE_CONFIG_DIR/settings.json" 2>/dev/null; then
            print_step "Deny rules configured"
        else
            print_warning "No deny rules configured"
        fi
    else
        print_warning "No Claude Code settings found"
    fi

    echo ""
}

# ═══════════════════════════════════════════════════════════════
# HARDENING FUNCTIONS
# ═══════════════════════════════════════════════════════════════

disable_telemetry_config() {
    print_header "🔒 Disabling Telemetry in Config"

    mkdir -p "$RTK_CONFIG_DIR"

    if [[ -f "$RTK_CONFIG_DIR/config.toml" ]]; then
        backup_file "$RTK_CONFIG_DIR/config.toml"

        # Check if [telemetry] section exists
        if grep -q "^\[telemetry\]" "$RTK_CONFIG_DIR/config.toml"; then
            # Update existing section
            if grep -q "^enabled = " "$RTK_CONFIG_DIR/config.toml"; then
                sed -i.bak '/^\[telemetry\]/,/^\[/{s/^enabled = .*/enabled = false/}' "$RTK_CONFIG_DIR/config.toml"
                print_step "Updated [telemetry] enabled = false"
            else
                sed -i.bak '/^\[telemetry\]/a\
enabled = false' "$RTK_CONFIG_DIR/config.toml"
                print_step "Added enabled = false to [telemetry] section"
            fi
        else
            # Add new section
            cat >> "$RTK_CONFIG_DIR/config.toml" << 'EOF'

[telemetry]
# Disable all telemetry pings (no data sent to external servers)
enabled = false
EOF
            print_step "Created [telemetry] section with enabled = false"
        fi
    else
        # Create new config
        cat > "$RTK_CONFIG_DIR/config.toml" << 'EOF'
# RTK Security Configuration
# Generated by rtk-security-hardening.sh
# Date: $(date +%Y-%m-%d)

[telemetry]
# Disable all telemetry pings (no data sent to external servers)
enabled = false

[tracking]
# Local tracking for 'rtk gain' stats (stays on your machine)
enabled = true
history_days = 90

[hooks]
# Exclude sensitive commands from auto-rewrite
exclude_commands = []

[display]
colors = true
emoji = true
max_width = 120

[limits]
grep_max_results = 200
grep_max_per_file = 25
status_max_files = 15
status_max_untracked = 10
passthrough_max_chars = 2000
EOF
        print_step "Created new config.toml with telemetry disabled"
    fi
}

disable_telemetry_env() {
    print_header "🔒 Setting Environment Variable"

    backup_file "$ZSHRC"

    # Check if already set
    if grep -q "RTK_TELEMETRY_DISABLED" "$ZSHRC" 2>/dev/null; then
        print_info "RTK_TELEMETRY_DISABLED already in $ZSHRC"
    else
        cat >> "$ZSHRC" << 'EOF'

# RTK Security - Disable Telemetry
export RTK_TELEMETRY_DISABLED=1
EOF
        print_step "Added RTK_TELEMETRY_DISABLED=1 to $ZSHRC"
    fi

    # Set for current session
    export RTK_TELEMETRY_DISABLED=1
    print_step "Set RTK_TELEMETRY_DISABLED=1 for current session"
}

disable_tracking() {
    print_header "🔒 Disabling Command Tracking"

    # Update config
    if [[ -f "$RTK_CONFIG_DIR/config.toml" ]]; then
        if grep -q "^\[tracking\]" "$RTK_CONFIG_DIR/config.toml"; then
            if grep -q "^enabled = " "$RTK_CONFIG_DIR/config.toml" | head -2; then
                sed -i.bak '/^\[tracking\]/,/^\[/{s/^enabled = .*/enabled = false/}' "$RTK_CONFIG_DIR/config.toml"
                print_step "Updated [tracking] enabled = false"
            else
                sed -i.bak '/^\[tracking\]/a\
enabled = false' "$RTK_CONFIG_DIR/config.toml"
                print_step "Added enabled = false to [tracking] section"
            fi
        else
            cat >> "$RTK_CONFIG_DIR/config.toml" << 'EOF'

[tracking]
# Disable local command tracking (no SQLite database)
enabled = false
EOF
            print_step "Created [tracking] section with enabled = false"
        fi
    fi

    # Redirect database to /dev/null
    if grep -q "RTK_DB_PATH" "$ZSHRC" 2>/dev/null; then
        print_info "RTK_DB_PATH already in $ZSHRC"
    else
        cat >> "$ZSHRC" << 'EOF'
export RTK_DB_PATH=/dev/null
EOF
        print_step "Added RTK_DB_PATH=/dev/null to $ZSHRC"
    fi

    export RTK_DB_PATH=/dev/null
    print_step "Set RTK_DB_PATH=/dev/null for current session"
}

clean_telemetry_files() {
    print_header "🧹 Cleaning Telemetry Files"

    local cleaned=0

    if [[ -f "$RTK_DATA_DIR/.telemetry_last_ping" ]]; then
        rm -f "$RTK_DATA_DIR/.telemetry_last_ping"
        print_step "Removed telemetry ping marker"
        ((cleaned++))
    fi

    if [[ -f "$RTK_DATA_DIR/.device_salt" ]]; then
        rm -f "$RTK_DATA_DIR/.device_salt"
        print_step "Removed device fingerprint salt"
        ((cleaned++))
    fi

    if [[ -f "$RTK_DATA_DIR/history.db" ]]; then
        backup_file "$RTK_DATA_DIR/history.db"
        rm -f "$RTK_DATA_DIR/history.db"*
        print_step "Removed tracking database"
        ((cleaned++))
    fi

    if [[ $cleaned -eq 0 ]]; then
        print_info "No telemetry files found to clean"
    else
        print_step "Cleaned $cleaned file(s)"
    fi
}

configure_command_exclusions() {
    print_header "🔒 Configuring Sensitive Command Exclusions"

    # Update config
    if [[ -f "$RTK_CONFIG_DIR/config.toml" ]]; then
        if grep -q "^\[hooks\]" "$RTK_CONFIG_DIR/config.toml"; then
            # Section exists, check if exclude_commands is set
            if grep -q "exclude_commands = \[\]" "$RTK_CONFIG_DIR/config.toml"; then
                # Replace empty array with sensitive commands
                sed -i.bak '/exclude_commands = \[\]/c\
exclude_commands = ["curl", "wget", "aws", "gcloud", "az", "kubectl", "docker", "ssh", "scp", "rsync"]' "$RTK_CONFIG_DIR/config.toml"
                print_step "Updated exclude_commands with sensitive commands"
            else
                print_info "exclude_commands already configured"
            fi
        else
            cat >> "$RTK_CONFIG_DIR/config.toml" << 'EOF'

[hooks]
# Exclude sensitive commands from RTK interception
exclude_commands = ["curl", "wget", "aws", "gcloud", "az", "kubectl", "docker", "ssh", "scp", "rsync"]
EOF
            print_step "Created [hooks] section with sensitive command exclusions"
        fi
    fi
}

configure_claude_deny_rules() {
    print_header "🔒 Configuring Claude Code Deny Rules"

    mkdir -p "$CLAUDE_CONFIG_DIR"

    if [[ -f "$CLAUDE_CONFIG_DIR/settings.json" ]]; then
        backup_file "$CLAUDE_CONFIG_DIR/settings.json"
        print_warning "Existing settings.json found - manual merge required"
        print_info "Backup created at: $BACKUP_DIR/settings.json"

        cat > "$CLAUDE_CONFIG_DIR/settings-rtk-security.json" << 'EOF'
{
  "permissions": {
    "deny": [
      "Bash(* --token *)",
      "Bash(* --password *)",
      "Bash(* --api-key *)",
      "Bash(* --secret *)",
      "Bash(* --bearer *)",
      "Bash(curl * -H *Authorization*)",
      "Bash(aws * --profile *)",
      "Bash(kubectl * --token *)",
      "Bash(docker login *)"
    ],
    "ask": [
      "Bash(git push --force *)",
      "Bash(git push -f *)",
      "Bash(rm -rf /*)",
      "Bash(npm publish *)",
      "Bash(cargo publish *)",
      "Bash(pip install *)",
      "Bash(sudo *)"
    ]
  }
}
EOF
        print_info "Created settings-rtk-security.json (merge manually)"
    else
        cat > "$CLAUDE_CONFIG_DIR/settings.json" << 'EOF'
{
  "permissions": {
    "deny": [
      "Bash(* --token *)",
      "Bash(* --password *)",
      "Bash(* --api-key *)",
      "Bash(* --secret *)",
      "Bash(* --bearer *)",
      "Bash(curl * -H *Authorization*)",
      "Bash(aws * --profile *)",
      "Bash(kubectl * --token *)",
      "Bash(docker login *)"
    ],
    "ask": [
      "Bash(git push --force *)",
      "Bash(git push -f *)",
      "Bash(rm -rf /*)",
      "Bash(npm publish *)",
      "Bash(cargo publish *)",
      "Bash(pip install *)",
      "Bash(sudo *)"
    ]
  }
}
EOF
        print_step "Created settings.json with deny/ask rules"
    fi
}

generate_security_report() {
    print_header "📊 Security Report"

    local report_file="$HOME/Documents/Workdir-LATAM/IDP/AGENTIC-SEC/rtk-security-report-$(date +%Y%m%d-%H%M%S).txt"

    {
        echo "═══════════════════════════════════════════════════════════════"
        echo "RTK SECURITY HARDENING REPORT"
        echo "═══════════════════════════════════════════════════════════════"
        echo "Date: $(date)"
        echo "User: $USER"
        echo "Host: $(hostname)"
        echo ""
        echo "═══════════════════════════════════════════════════════════════"
        echo "CONFIGURATION STATUS"
        echo "═══════════════════════════════════════════════════════════════"
        echo ""
        echo "1. Telemetry Configuration:"
        echo "   - Config file: $RTK_CONFIG_DIR/config.toml"
        grep -A 2 "\[telemetry\]" "$RTK_CONFIG_DIR/config.toml" 2>/dev/null | sed 's/^/     /'
        echo ""
        echo "2. Environment Variables:"
        echo "   - RTK_TELEMETRY_DISABLED: ${RTK_TELEMETRY_DISABLED:-NOT SET}"
        echo "   - RTK_DB_PATH: ${RTK_DB_PATH:-NOT SET}"
        echo ""
        echo "3. Files Status:"
        echo "   - Telemetry ping: $(ls -la "$RTK_DATA_DIR/.telemetry_last_ping" 2>/dev/null || echo 'NOT FOUND (✓)')"
        echo "   - Device salt: $(ls -la "$RTK_DATA_DIR/.device_salt" 2>/dev/null || echo 'NOT FOUND (✓)')"
        echo "   - History DB: $(ls -la "$RTK_DATA_DIR/history.db" 2>/dev/null || echo 'NOT FOUND (✓)')"
        echo ""
        echo "4. Command Exclusions:"
        grep "exclude_commands" "$RTK_CONFIG_DIR/config.toml" 2>/dev/null | sed 's/^/   /'
        echo ""
        echo "5. Claude Code Deny Rules:"
        if [[ -f "$CLAUDE_CONFIG_DIR/settings.json" ]]; then
            jq -r '.permissions.deny[]' "$CLAUDE_CONFIG_DIR/settings.json" 2>/dev/null | sed 's/^/   - /' || echo "   (JSON parse error)"
        else
            echo "   NOT CONFIGURED"
        fi
        echo ""
        echo "═══════════════════════════════════════════════════════════════"
        echo "BACKUP LOCATION"
        echo "═══════════════════════════════════════════════════════════════"
        echo "$BACKUP_DIR"
        echo ""
    } | tee "$report_file"

    print_step "Report saved to: $report_file"
}

# ═══════════════════════════════════════════════════════════════
# MAIN MENU
# ═══════════════════════════════════════════════════════════════

show_menu() {
    clear
    echo -e "${BLUE}"
    cat << 'EOF'
╔════════════════════════════════════════════════════════════════╗
║                                                                ║
║           RTK SECURITY HARDENING TOOL v1.0.0                  ║
║                                                                ║
╚════════════════════════════════════════════════════════════════╝
EOF
    echo -e "${NC}"

    echo "Select an option:"
    echo ""
    echo "  1) 🔍 Audit current security state"
    echo "  2) 🔒 Full hardening (recommended)"
    echo "  3) 🔐 Disable telemetry only"
    echo "  4) 🚫 Disable tracking only"
    echo "  5) 🧹 Clean telemetry files"
    echo "  6) ⚙️  Configure command exclusions"
    echo "  7) 🛡️  Configure Claude Code deny rules"
    echo "  8) 📊 Generate security report"
    echo "  9) ❌ Exit"
    echo ""
    read -p "Enter choice [1-9]: " choice

    case $choice in
        1) audit_current_state ;;
        2) full_hardening ;;
        3) disable_telemetry_config; disable_telemetry_env ;;
        4) disable_tracking ;;
        5) clean_telemetry_files ;;
        6) configure_command_exclusions ;;
        7) configure_claude_deny_rules ;;
        8) generate_security_report ;;
        9) exit 0 ;;
        *) echo "Invalid option"; sleep 2 ;;
    esac

    echo ""
    read -p "Press Enter to continue..."
}

full_hardening() {
    print_header "🔒 FULL SECURITY HARDENING"

    audit_current_state
    disable_telemetry_config
    disable_telemetry_env
    disable_tracking
    clean_telemetry_files
    configure_command_exclusions
    configure_claude_deny_rules
    generate_security_report

    print_header "✅ HARDENING COMPLETE"

    echo -e "${GREEN}All security measures applied successfully!${NC}"
    echo ""
    echo -e "${YELLOW}⚠  IMPORTANT: Restart your terminal to apply changes${NC}"
    echo ""
    echo "Run the following command to activate changes now:"
    echo "  source $ZSHRC"
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# MAIN EXECUTION
# ═══════════════════════════════════════════════════════════════

main() {
    # Check if RTK is installed
    if ! command -v rtk &> /dev/null; then
        print_error "RTK is not installed. Install it first with:"
        echo "  brew install rtk"
        echo "  # or"
        echo "  cargo install rtk"
        exit 1
    fi

    # Check if running in interactive mode
    if [[ "${1:-}" == "--auto" ]]; then
        full_hardening
    elif [[ "${1:-}" == "--audit" ]]; then
        audit_current_state
    elif [[ "${1:-}" == "--clean" ]]; then
        clean_telemetry_files
    elif [[ "${1:-}" == "--help" ]]; then
        echo "RTK Security Hardening Tool"
        echo ""
        echo "Usage:"
        echo "  $0             Interactive menu"
        echo "  $0 --auto      Full automatic hardening"
        echo "  $0 --audit     Security audit only"
        echo "  $0 --clean     Clean telemetry files only"
        echo "  $0 --help      Show this help"
        exit 0
    else
        # Interactive mode
        while true; do
            show_menu
        done
    fi
}

main "$@"
