// RTK auto-rewrite plugin for OpenCode
// Transparently rewrites raw commands to their rtk equivalents.
// Uses tool.execute.before to modify Bash commands before execution.
//
// Equivalent to hooks/rtk-rewrite.sh for Claude Code.

export const RtkRewrite = async () => {
  return {
    "tool.execute.before": async (
      input: { tool: string; args: Record<string, unknown> },
      output: { args: Record<string, unknown> },
    ) => {
      if (input.tool !== "bash") return;

      const command = output.args.command;
      if (typeof command !== "string" || !command) return;

      const rewritten = rewriteCommand(command);
      if (rewritten) {
        output.args.command = rewritten;
      }
    },
  };
};

/**
 * Attempt to rewrite a command to use rtk.
 * Returns the rewritten command, or null if no rewrite is needed.
 */
function rewriteCommand(cmd: string): string | null {
  // Skip if already using rtk as the actual command (not just in a path)
  // Check the basename of the first word before the first space
  const firstWord = cmd.split(/\s/)[0];
  const basename = firstWord.split("/").pop();
  if (basename === "rtk") return null;

  // Skip heredocs
  if (cmd.includes("<<")) return null;

  // Strip leading env var assignments for matching
  // e.g., "TEST_SESSION_ID=2 npx playwright test" → match against "npx playwright test"
  const envPrefixMatch = cmd.match(/^(?:[A-Za-z_][A-Za-z0-9_]*=[^\s]*\s+)+/);
  const envPrefix = envPrefixMatch ? envPrefixMatch[0] : "";
  const matchCmd = envPrefix ? cmd.slice(envPrefix.length) : cmd;
  const cmdBody = matchCmd; // The part after env prefix

  // --- Git commands ---
  if (/^git\s/.test(matchCmd)) {
    // Skip git commands with -C or -c flags (not yet supported by RTK)
    if (/git\s+.*(-C|-c)\s/.test(matchCmd)) {
      return null;
    }
    
    // Strip git options (--no-pager, etc.) for subcommand matching
    const gitSub = matchCmd
      .replace(/^git\s+/, "")
      .replace(/--[a-z-]+=\S+\s*/g, "")
      .replace(/--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*/g, "")
      .trimStart();

    if (
      /^(status|diff|log|add|commit|push|pull|branch|fetch|stash|show)(\s|$)/.test(gitSub)
    ) {
      return `${envPrefix}rtk ${cmdBody}`;
    }
    return null;
  }

  // --- GitHub CLI ---
  if (/^gh\s+(pr|issue|run|api|release)(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^gh /, "rtk gh ")}`;
  }

  // --- Cargo ---
  if (/^cargo\s/.test(matchCmd)) {
    const cargoSub = matchCmd
      .replace(/^cargo\s+(\+\S+\s+)?/, "");
    if (/^(test|build|clippy|check|install|fmt)(\s|$)/.test(cargoSub)) {
      return `${envPrefix}rtk ${cmdBody}`;
    }
    return null;
  }

  // --- File operations ---
  if (/^cat\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^cat /, "rtk read ")}`;
  }
  if (/^(rg|grep)\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(rg|grep) /, "rtk grep ")}`;
  }
  if (/^ls(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^ls/, "rtk ls")}`;
  }
  if (/^tree(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^tree/, "rtk tree")}`;
  }
  if (/^find\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^find /, "rtk find ")}`;
  }
  if (/^diff\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^diff /, "rtk diff ")}`;
  }
  if (/^head\s+/.test(matchCmd)) {
    // head -N file → rtk read file --max-lines N
    const dashN = matchCmd.match(/^head\s+-(\d+)\s+(.+)$/);
    if (dashN) {
      return `${envPrefix}rtk read ${dashN[2]} --max-lines ${dashN[1]}`;
    }
    const longLines = matchCmd.match(/^head\s+--lines=(\d+)\s+(.+)$/);
    if (longLines) {
      return `${envPrefix}rtk read ${longLines[2]} --max-lines ${longLines[1]}`;
    }
    return null;
  }

  // --- JS/TS tooling ---
  if (/^(pnpm\s+)?(npx\s+)?vitest(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(pnpm )?(npx )?vitest( run)?/, "rtk vitest run")}`;
  }
  if (/^pnpm\s+test(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pnpm test/, "rtk vitest run")}`;
  }
  if (/^npm\s+test(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^npm test/, "rtk npm test")}`;
  }
  if (/^npm\s+run\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^npm run /, "rtk npm ")}`;
  }
  if (/^(npx\s+)?vue-tsc(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?vue-tsc/, "rtk tsc")}`;
  }
  if (/^pnpm\s+tsc(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pnpm tsc/, "rtk tsc")}`;
  }
  if (/^(npx\s+)?tsc(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?tsc/, "rtk tsc")}`;
  }
  if (/^pnpm\s+lint(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pnpm lint/, "rtk lint")}`;
  }
  if (/^(npx\s+)?eslint(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?eslint/, "rtk lint")}`;
  }
  if (/^(npx\s+)?prettier(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?prettier/, "rtk prettier")}`;
  }
  if (/^(npx\s+)?playwright(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?playwright/, "rtk playwright")}`;
  }
  if (/^pnpm\s+playwright(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pnpm playwright/, "rtk playwright")}`;
  }
  if (/^(npx\s+)?prisma(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^(npx )?prisma/, "rtk prisma")}`;
  }

  // --- Containers ---
  if (/^docker\s/.test(matchCmd)) {
    if (/^docker\s+compose(\s|$)/.test(matchCmd)) {
      return `${envPrefix}${cmdBody.replace(/^docker /, "rtk docker ")}`;
    }
    const dockerSub = matchCmd
      .replace(/^docker\s+/, "")
      .replace(/(-H|--context|--config)\s+\S+\s*/g, "")
      .replace(/--[a-z-]+=\S+\s*/g, "")
      .trimStart();
    if (/^(ps|images|logs|run|build|exec)(\s|$)/.test(dockerSub)) {
      return `${envPrefix}${cmdBody.replace(/^docker /, "rtk docker ")}`;
    }
    return null;
  }
  if (/^kubectl\s/.test(matchCmd)) {
    const kubeSub = matchCmd
      .replace(/^kubectl\s+/, "")
      .replace(/(--context|--kubeconfig|--namespace|-n)\s+\S+\s*/g, "")
      .replace(/--[a-z-]+=\S+\s*/g, "")
      .trimStart();
    if (/^(get|logs|describe|apply)(\s|$)/.test(kubeSub)) {
      return `${envPrefix}${cmdBody.replace(/^kubectl /, "rtk kubectl ")}`;
    }
    return null;
  }

  // --- Network ---
  if (/^curl\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^curl /, "rtk curl ")}`;
  }
  if (/^wget\s+/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^wget /, "rtk wget ")}`;
  }

  // --- pnpm package management ---
  if (/^pnpm\s+(list|ls|outdated)(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pnpm /, "rtk pnpm ")}`;
  }

  // --- Python tooling ---
  if (/^pytest(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pytest/, "rtk pytest")}`;
  }
  if (/^python\s+-m\s+pytest(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^python -m pytest/, "rtk pytest")}`;
  }
  if (/^ruff\s+(check|format)(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^ruff /, "rtk ruff ")}`;
  }
  if (/^pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^pip /, "rtk pip ")}`;
  }
  if (/^uv\s+pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^uv pip /, "rtk pip ")}`;
  }

  // --- Go tooling ---
  if (/^go\s+test(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^go test/, "rtk go test")}`;
  }
  if (/^go\s+build(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^go build/, "rtk go build")}`;
  }
  if (/^go\s+vet(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^go vet/, "rtk go vet")}`;
  }
  if (/^golangci-lint(\s|$)/.test(matchCmd)) {
    return `${envPrefix}${cmdBody.replace(/^golangci-lint/, "rtk golangci-lint")}`;
  }

  return null;
}
