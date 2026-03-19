#!/usr/bin/env node
/**
 * RTK Cursor Agent hook — rewrites shell commands to use rtk for token savings.
 * Cross-platform Node.js version (Windows/Mac/Linux)
 *
 * This is a thin delegating hook: all rewrite logic lives in `rtk rewrite`,
 * which is the single source of truth (src/discover/registry.rs).
 */

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');

// Platform-specific executable check
function commandExists(cmd) {
    try {
        const platform = os.platform();
        if (platform === 'win32') {
            execSync(`where ${cmd}`, { stdio: 'ignore' });
        } else {
            execSync(`command -v ${cmd}`, { stdio: 'ignore' });
        }
        return true;
    } catch {
        return false;
    }
}

// Get rtk version
function getRtkVersion() {
    try {
        const output = execSync('rtk --version', { encoding: 'utf8' });
        const match = output.match(/(\d+)\.(\d+)\.(\d+)/);
        if (match) {
            return { major: parseInt(match[1]), minor: parseInt(match[2]), full: match[0] };
        }
    } catch {}
    return null;
}

// Main hook function
function preToolUse(context, toolName, toolInput) {
    // Only process shell commands
    if (toolName !== 'Bash' && toolName !== 'shell') {
        return;
    }

    const command = toolInput?.command;
    if (!command || typeof command !== 'string') {
        return;
    }

    // Check if rtk is available
    if (!commandExists('rtk')) {
        console.error('[rtk] WARNING: rtk is not installed or not in PATH.');
        console.error('[rtk] Install: https://github.com/rtk-ai/rtk#installation');
        return;
    }

    // Version guard: rtk rewrite was added in 0.23.0
    const version = getRtkVersion();
    if (version && version.major === 0 && version.minor < 23) {
        console.error(`[rtk] WARNING: rtk ${version.full} is too old (need >= 0.23.0).`);
        console.error('[rtk] Upgrade: cargo install rtk');
        return;
    }

    // Delegate all rewrite logic to the Rust binary
    let rewritten;
    try {
        rewritten = execSync(`rtk rewrite ${JSON.stringify(command)}`, {
            encoding: 'utf8',
            stdio: ['pipe', 'pipe', 'pipe']
        }).trim();
    } catch {
        // rtk rewrite exits 1 when there's no rewrite — pass through silently
        return;
    }

    // No change — nothing to do
    if (rewritten === command) {
        return;
    }

    // Return the rewritten command
    return {
        permission: 'allow',
        permissionDecisionReason: 'RTK auto-rewrite',
        updatedInput: {
            ...toolInput,
            command: rewritten
        }
    };
}

module.exports = { preToolUse };
