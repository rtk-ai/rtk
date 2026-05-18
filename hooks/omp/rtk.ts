// RTK - Rust Token Killer
// OMP extension: rewrite bash tool calls through `rtk rewrite`.
//
// This is a thin delegating extension. All rewrite logic lives in RTK's Rust
// registry via `rtk rewrite`, which remains the single source of truth.

type BashToolCallEvent = {
  toolName: string;
  input: { command: string };
};

type ExtensionContext = {
  ui: {
    setStatus(key: string, text: string | undefined): void;
  };
};

type ExtensionAPI = {
  setLabel(label: string): void;
  on(
    event: "session_start",
    handler: (
      event: unknown,
      ctx: ExtensionContext,
    ) => Promise<void> | void,
  ): void;
  on(
    event: "tool_call",
    handler: (
      event: BashToolCallEvent,
    ) => Promise<void> | void,
  ): void;
};

type RewriteDecision = { kind: "rewrite"; rewritten: string } | { kind: "skip" };

function readText(stream: ReadableStream<Uint8Array> | null | undefined, name: string): Promise<string> {
  if (!stream) {
    throw new Error(`rtk rewrite ${name} stream was unavailable`);
  }
  return new Response(stream).text().then((text) => text.trim());
}

async function rewriteWithRtk(command: string): Promise<RewriteDecision> {
  const proc = Bun.spawn(["rtk", "rewrite", command], {
    stdout: "pipe",
    stderr: "pipe",
  });

  const [exitCode, stdout] = await Promise.all([
    proc.exited,
    readText(proc.stdout, "stdout"),
    proc.stderr?.cancel(),
  ]);

  switch (exitCode) {
    case 0:
    case 3:
      if (!stdout || stdout === command) {
        return { kind: "skip" };
      }
      return { kind: "rewrite", rewritten: stdout };
    default:
      return { kind: "skip" };
  }
}

export default function rtkOmpExtension(pi: ExtensionAPI) {
  pi.setLabel("RTK");

  const hasRtk = Boolean(Bun.which("rtk"));

  if (!hasRtk) {
    pi.on("session_start", (_event, ctx) => {
      ctx.ui.setStatus("rtk", "RTK extension disabled: rtk binary not found in PATH.");
    });
    return;
  }

  pi.on("tool_call", async (event) => {
    if (event.toolName !== "bash") {
      return;
    }

    const original = event.input.command;
    if (original.trim() === "") {
      return;
    }

    try {
      const decision = await rewriteWithRtk(original);
      if (decision.kind === "skip") {
        return;
      }

      event.input.command = decision.rewritten;
    } catch {
      return;
    }
  });
}
