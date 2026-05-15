// RTK - Rust Token Killer
// OMP extension: rewrite bash tool calls through `rtk rewrite`.
//
// This is a thin delegating extension. All rewrite logic lives in RTK's Rust
// registry via `rtk rewrite`, which remains the single source of truth.

type BashToolCallEvent = {
  toolName: string;
  input: { command: string };
};

type ExtensionAPI = {
  setLabel(label: string): void;
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
      if (!stdout) {
        return { kind: "skip" };
      }
      return stdout !== command
        ? { kind: "rewrite", rewritten: stdout }
        : { kind: "skip" };
    case 1:
      return { kind: "skip" };
    case 2:
      return { kind: "skip" };
    default:
      return { kind: "skip" };
  }
}

export default function rtkOmpExtension(pi: ExtensionAPI) {
  pi.setLabel("RTK");

  const hasRtk = Boolean(Bun.which("rtk"));

  pi.on("tool_call", async (event) => {
    if (event.toolName !== "bash") {
      return;
    }

    if (!hasRtk) {
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
