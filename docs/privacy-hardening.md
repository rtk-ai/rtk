# Privacy Hardening Guide

Quick privacy configuration for RTK users via automated script.

## Quick Start

```bash
# Automated full hardening
./scripts/rtk-security-hardening.sh --auto
source ~/.zshrc
```

## What Gets Configured

| Feature | Default | After Hardening |
|---------|---------|-----------------|
| Telemetry | Enabled | **Disabled** (config + env var) |
| Device Fingerprinting | Active | **Removed** |
| Command Tracking | Enabled | **Disabled** (optional) |
| Sensitive Commands | Proxied | **Excluded** (curl, aws, kubectl, etc.) |

## Usage

### Interactive Menu
```bash
./scripts/rtk-security-hardening.sh
```

Options: Audit, full hardening, selective hardening, security report.

### Command-Line Flags
```bash
./scripts/rtk-security-hardening.sh --auto    # Full hardening
./scripts/rtk-security-hardening.sh --audit   # Security audit only
./scripts/rtk-security-hardening.sh --clean   # Clean telemetry files
./scripts/rtk-security-hardening.sh --help    # Show usage
```

## Configuration Details

### 1. Disable Telemetry
- Sets `~/.config/rtk/config.toml` → `[telemetry] enabled = false`
- Exports `RTK_TELEMETRY_DISABLED=1` in `~/.zshrc`
- **Effect**: No data sent to external servers

### 2. Disable Tracking (Optional)
- Sets `~/.config/rtk/config.toml` → `[tracking] enabled = false`
- Exports `RTK_DB_PATH=/dev/null` in `~/.zshrc`
- **Effect**: Commands not persisted to SQLite
- **Trade-off**: `rtk gain` stops working

### 3. Command Exclusions
Excludes sensitive commands from RTK proxy:
```toml
[hooks]
exclude_commands = ["curl", "wget", "aws", "gcloud", "az", "kubectl", "docker", "ssh", "scp", "rsync"]
```

### 4. Claude Code Deny Rules (Optional)
Creates `~/.claude/settings.json` with:
```json
{
  "permissions": {
    "deny": ["Bash(* --token *)", "Bash(* --password *)", "Bash(* --api-key *)"]
  }
}
```

## Verification

```bash
# Check environment variables
echo $RTK_TELEMETRY_DISABLED  # Should be: 1
echo $RTK_DB_PATH              # Should be: /dev/null

# Verify telemetry disabled
cat ~/.config/rtk/config.toml | grep "enabled = false"

# Test RTK
rtk git log -3
```

## Safety Features

- **Automatic backups**: Creates `~/.rtk-backup-<timestamp>/` before changes
- **Idempotent**: Safe to run multiple times
- **Non-destructive**: Appends to config files
- **No sudo required**: All user-space operations

## Manual Configuration

If you prefer manual setup:

```bash
# Disable telemetry
mkdir -p ~/.config/rtk
cat >> ~/.config/rtk/config.toml << 'EOF'
[telemetry]
enabled = false
EOF
echo 'export RTK_TELEMETRY_DISABLED=1' >> ~/.zshrc

# Disable tracking
cat >> ~/.config/rtk/config.toml << 'EOF'
[tracking]
enabled = false
EOF
echo 'export RTK_DB_PATH=/dev/null' >> ~/.zshrc

# Apply changes
source ~/.zshrc
```

## Privacy vs Functionality

| Setting | Privacy Gain | Functionality Loss |
|---------|--------------|-------------------|
| Telemetry OFF | ✅ No external data | ⚠️ Project loses metrics |
| Tracking OFF | ✅ No command history | ⚠️ `rtk gain` unavailable |
| Commands Excluded | ✅ No credential logging | ⚠️ No optimization for those commands |

**Recommendation**: Disable telemetry, keep tracking if you use `rtk gain`.

## Troubleshooting

**`rtk gain` shows no data**
- Expected if tracking is disabled
- To re-enable: Set `tracking.enabled = true` in config

**Telemetry files reappear**
- Run hardening again: `./scripts/rtk-security-hardening.sh --auto`

**RTK not working**
- Verify installation: `rtk --version`
- Check config syntax: `cat ~/.config/rtk/config.toml`
- Reload shell: `source ~/.zshrc`

## Source Code

- Telemetry implementation: `src/core/telemetry.rs`
- Tracking implementation: `src/core/tracking.rs`
- Configuration schema: `src/core/config.rs`
