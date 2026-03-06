/**
 * RTK Rewrite Plugin for OpenClaw
 *
 * Transparently rewrites exec tool commands to RTK equivalents
 * before execution, achieving 60-90% LLM token savings.
 */

// ---------------------------------------------------------------------------
// Rewrite rules
// ---------------------------------------------------------------------------

interface RewriteRule {
  pattern: RegExp;
  replacement: string;
}

const REWRITE_RULES: RewriteRule[] = [
  // Git
  { pattern: /^git\s+status(\s|$)/, replacement: "rtk git status$1" },
  { pattern: /^git\s+diff(\s|$)/, replacement: "rtk git diff$1" },
  { pattern: /^git\s+log(\s|$)/, replacement: "rtk git log$1" },
  { pattern: /^git\s+add(\s|$)/, replacement: "rtk git add$1" },
  { pattern: /^git\s+commit(\s|$)/, replacement: "rtk git commit$1" },
  { pattern: /^git\s+push(\s|$)/, replacement: "rtk git push$1" },
  { pattern: /^git\s+pull(\s|$)/, replacement: "rtk git pull$1" },
  { pattern: /^git\s+branch(\s|$)/, replacement: "rtk git branch$1" },
  { pattern: /^git\s+fetch(\s|$)/, replacement: "rtk git fetch$1" },
  { pattern: /^git\s+show(\s|$)/, replacement: "rtk git show$1" },

  // GitHub CLI
  { pattern: /^gh\s+(pr|issue|run)(\s|$)/, replacement: "rtk gh $1$2" },

  // File operations
  { pattern: /^(rg|grep)\s+/, replacement: "rtk grep " },
  { pattern: /^ls(\s|$)/, replacement: "rtk ls$1" },
  { pattern: /^find\s+/, replacement: "rtk find " },

  // JS/TS tooling
  { pattern: /^(pnpm\s+)?vitest(\s|$)/, replacement: "rtk vitest run$2" },
  { pattern: /^pnpm\s+test(\s|$)/, replacement: "rtk vitest run$1" },
  { pattern: /^(npx\s+)?tsc(\s|$)/, replacement: "rtk tsc$2" },
  { pattern: /^(npx\s+)?eslint(\s|$)/, replacement: "rtk lint$2" },
  { pattern: /^pnpm\s+lint(\s|$)/, replacement: "rtk lint$1" },
  { pattern: /^(npx\s+)?prisma(\s|$)/, replacement: "rtk prisma$2" },

  // npm/pnpm
  { pattern: /^npm\s+test(\s|$)/, replacement: "rtk test npm test$1" },
  { pattern: /^pnpm\s+(list|ls|outdated)(\s|$)/, replacement: "rtk pnpm $1$2" },

  // Containers
  { pattern: /^docker\s+(ps|images|logs)(\s|$)/, replacement: "rtk docker $1$2" },
  { pattern: /^kubectl\s+(get|logs)(\s|$)/, replacement: "rtk kubectl $1$2" },

  // Python
  { pattern: /^pytest(\s|$)/, replacement: "rtk pytest$1" },
  { pattern: /^python\s+-m\s+pytest(\s|$)/, replacement: "rtk pytest$1" },

  // Go
  { pattern: /^go\s+test(\s|$)/, replacement: "rtk go test$1" },
  { pattern: /^go\s+build(\s|$)/, replacement: "rtk go build$1" },
];

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

function shouldSkip(cmd: string): boolean {
  // Already using rtk
  if (/^rtk\s/.test(cmd)) return true;
  // Heredocs, pipes, compound commands — too complex to safely rewrite
  if (cmd.includes("<<") || cmd.includes("|") || cmd.includes("&&") || cmd.includes(";")) return true;
  return false;
}

function tryRewrite(cmd: string): string | null {
  if (shouldSkip(cmd)) return null;
  for (const rule of REWRITE_RULES) {
    if (rule.pattern.test(cmd)) {
      return cmd.replace(rule.pattern, rule.replacement);
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

export default function register(api: any) {
  const pluginConfig = api.config ?? {};
  const enabled = pluginConfig.enabled !== false;
  const verbose = pluginConfig.verbose === true;

  if (!enabled) return;

  api.on(
    "before_tool_call",
    (event: { toolName: string; params: Record<string, unknown> }) => {
      if (event.toolName !== "exec") return;

      const command = event.params?.command;
      if (typeof command !== "string") return;

      const rewritten = tryRewrite(command);
      if (!rewritten) return;

      if (verbose) {
        console.log(`[rtk-rewrite] ${command} → ${rewritten}`);
      }

      // Return updated params per the PluginHookBeforeToolCallResult type
      return { params: { ...event.params, command: rewritten } };
    },
    { priority: 10 }
  );

  if (verbose) {
    console.log(`[rtk-rewrite] Registered (${REWRITE_RULES.length} rules)`);
  }
}

export { tryRewrite, shouldSkip, REWRITE_RULES };
