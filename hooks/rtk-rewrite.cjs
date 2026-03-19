/**
 * RTK Auto-Rewrite Hook for Claude Code
 * Cross-platform Node.js version (Windows/Mac/Linux)
 *
 * This hook transparently rewrites raw commands to their rtk equivalents
 * by intercepting PreToolUse events for Bash commands.
 */

function preToolUse(context, toolName, toolInput) {
    // Only process Bash commands
    if (toolName !== 'Bash') {
        return;
    }

    const command = toolInput?.command;
    if (!command || typeof command !== 'string') {
        return;
    }

    // Skip if already using rtk
    if (/^rtk\s/.test(command) || /\/rtk\s/.test(command)) {
        return;
    }

    // Skip commands with heredocs (they break command parsing)
    if (command.includes('<<')) {
        return;
    }

    // Extract the first meaningful command (before pipes, &&, ||)
    // We only rewrite if the FIRST command in a chain matches.
    const firstCmd = command.split(/&&|\|\||\|/)[0].trim();

    // Command rewrite rules
    // Each entry is a [pattern, replacement] pair
    const rewrites = [
        // --- Git commands ---
        [/^git\s+status/, 'rtk git status'],
        [/^git\s+diff/, 'rtk git diff'],
        [/^git\s+log/, 'rtk git log'],
        [/^git\s+add/, 'rtk git add'],
        [/^git\s+commit/, 'rtk git commit'],
        [/^git\s+push/, 'rtk git push'],
        [/^git\s+pull/, 'rtk git pull'],
        [/^git\s+branch/, 'rtk git branch'],
        [/^git\s+fetch/, 'rtk git fetch'],
        [/^git\s+stash/, 'rtk git stash'],
        [/^git\s+show/, 'rtk git show'],

        // --- GitHub CLI ---
        [/^gh\s+/, 'rtk gh '],

        // --- Cargo / Rust ---
        [/^cargo\s+test/, 'rtk cargo test'],
        [/^cargo\s+build/, 'rtk cargo build'],
        [/^cargo\s+check/, 'rtk cargo check'],
        [/^cargo\s+clippy/, 'rtk cargo clippy'],

        // --- File operations ---
        [/^cat\s+/, 'rtk read '],
        [/^(rg|grep)\s+/, 'rtk grep '],
        [/^ls\s?$/, 'rtk ls'],

        // --- JavaScript/TypeScript tooling ---
        [/^(pnpm\s+)?vitest\s/, 'rtk vitest run'],
        [/^pnpm\s+test/, 'rtk vitest run'],
        [/^(pnpm\s+)?tsc/, 'rtk tsc'],
        [/^(npx\s+)?tsc/, 'rtk tsc'],
        [/^pnpm\s+lint/, 'rtk lint'],
        [/^(npx\s+)?eslint\s/, 'rtk lint'],
        [/^(npx\s+)?prettier\s/, 'rtk prettier'],
        [/^(npx\s+)?playwright\s/, 'rtk playwright'],
        [/^pnpm\s+playwright/, 'rtk playwright'],
        [/^(npx\s+)?prisma\s/, 'rtk prisma'],

        // --- Package managers ---
        [/^pnpm\s+(list|ls|outdated)/, 'rtk pnpm '],
        [/^pnpm\s+install/, 'rtk pnpm install'],

        // --- Containers & orchestration ---
        [/^docker\s+(ps|images|logs)/, 'rtk docker '],
        [/^kubectl\s+(get|logs)/, 'rtk kubectl '],

        // --- Network ---
        [/^curl\s+/, 'rtk curl '],

        // --- Python tooling ---
        [/^pytest\s/, 'rtk pytest'],
        [/^python\s+-m\s+pytest/, 'rtk pytest'],
        [/^ruff\s+/, 'rtk ruff '],
        [/^pip\s+(list|outdated|install|show)/, 'rtk pip '],
        [/^uv\s+pip\s+(list|outdated|install|show)/, 'rtk pip '],

        // --- Go tooling ---
        [/^go\s+test/, 'rtk go test'],
        [/^go\s+build/, 'rtk go build'],
        [/^go\s+vet/, 'rtk go vet'],
        [/^golangci-lint/, 'rtk golangci-lint'],
    ];

    // Find and apply the first matching rewrite
    for (const [pattern, replacement] of rewrites) {
        if (pattern.test(firstCmd)) {
            const rewritten = command.replace(pattern, replacement);

            return {
                permissionDecision: 'allow',
                permissionDecisionReason: 'RTK auto-rewrite',
                updatedInput: {
                    ...toolInput,
                    command: rewritten
                }
            };
        }
    }

    // No rewrite needed, let command pass through unchanged
    return undefined;
}

module.exports = { preToolUse };
