import { beforeEach, describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { createItem, logFocusSession, updateItem, type ItemInput } from "./boardApi";

vi.mock("../../lib/supabase", () => ({
  supabase: {
    from: vi.fn(),
    rpc: vi.fn(),
  },
}));

describe("board item persistence", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("creates an item with Supabase column names and maps the returned row", async () => {
    const insert = vi.fn();
    vi.mocked(supabase.from).mockReturnValue(chain({ insert, single: { data: row(), error: null } }));

    await expect(createItem(input())).resolves.toEqual(
      expect.objectContaining({
        id: "item-1",
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "inbox",
        title: "Plan launch",
        tags: ["launch"],
        sortOrder: 2,
      }),
    );

    expect(supabase.from).toHaveBeenCalledWith("items");
    expect(insert).toHaveBeenCalledWith({
      workspace_id: "workspace-1",
      board_id: "board-1",
      column_id: "inbox",
      title: "Plan launch",
      description: "Draft the rollout",
      type: "content",
      tags: ["launch"],
      priority: "high",
      status: "Inbox",
      scheduled_date: "2026-05-08",
      due_date: "2026-05-10",
      estimate_minutes: 75,
      progress_mode: "focus_time",
      progress_value: null,
      created_by: "user-1",
    });
  });

  it("updates an item inside the current workspace and board", async () => {
    const update = vi.fn();
    const eq = vi.fn();
    vi.mocked(supabase.from).mockReturnValue(chain({ update, eq, single: { data: row({ title: "Updated" }), error: null } }));

    await expect(updateItem("item-1", { ...input(), title: "Updated" })).resolves.toEqual(
      expect.objectContaining({ id: "item-1", title: "Updated" }),
    );

    expect(update).toHaveBeenCalledWith(
      expect.objectContaining({
        workspace_id: "workspace-1",
        board_id: "board-1",
        title: "Updated",
        due_date: "2026-05-10",
      }),
    );
    expect(update).not.toHaveBeenCalledWith(expect.objectContaining({ created_by: "user-1" }));
    expect(eq).toHaveBeenNthCalledWith(1, "id", "item-1");
    expect(eq).toHaveBeenNthCalledWith(2, "workspace_id", "workspace-1");
    expect(eq).toHaveBeenNthCalledWith(3, "board_id", "board-1");
  });

  it("logs a focus session and increments item focus minutes through the rpc", async () => {
    const insertFocus = vi.fn().mockResolvedValue({ error: null });
    vi.mocked(supabase.from).mockImplementation((table: string) => {
      if (table === "focus_sessions") return { insert: insertFocus } as never;
      throw new Error(`Unexpected table: ${table}`);
    });
    vi.mocked(supabase.rpc).mockResolvedValue({ error: null } as never);

    await logFocusSession(focusInput());

    expect(insertFocus).toHaveBeenCalledWith({
      workspace_id: "workspace-1",
      item_id: "item-1",
      goal_id: "goal-1",
      started_at: "2026-05-06T09:30:00.000Z",
      ended_at: "2026-05-06T10:00:00.000Z",
      duration_minutes: 30,
    });
    expect(supabase.rpc).toHaveBeenCalledWith("increment_item_focus_minutes", {
      target_item_id: "item-1",
      minutes_to_add: 30,
    });
  });

  it("falls back to a scoped direct item update when the focus rpc is unavailable", async () => {
    const insertFocus = vi.fn().mockResolvedValue({ error: null });
    const updateEqWorkspace = vi.fn().mockResolvedValue({ error: null });
    const updateEqItem = vi.fn(() => ({ eq: updateEqWorkspace }));
    const update = vi.fn(() => ({ eq: updateEqItem }));
    let itemTableCalls = 0;

    vi.mocked(supabase.from).mockImplementation((table: string) => {
      if (table === "focus_sessions") return { insert: insertFocus } as never;
      if (table === "items") {
        itemTableCalls += 1;
        if (itemTableCalls === 1) {
          return chain({ single: { data: { focus_minutes_total: 40 }, error: null } });
        }
        return { update } as never;
      }
      throw new Error(`Unexpected table: ${table}`);
    });
    vi.mocked(supabase.rpc).mockResolvedValue({ error: new Error("missing rpc") } as never);

    await logFocusSession(focusInput({ durationMinutes: 20 }));

    expect(update).toHaveBeenCalledWith({ focus_minutes_total: 60 });
    expect(updateEqItem).toHaveBeenCalledWith("id", "item-1");
    expect(updateEqWorkspace).toHaveBeenCalledWith("workspace_id", "workspace-1");
  });
});

function input(overrides: Partial<ItemInput> = {}): ItemInput {
  return {
    workspaceId: "workspace-1",
    boardId: "board-1",
    columnId: "inbox",
    title: "Plan launch",
    description: "Draft the rollout",
    type: "content",
    tags: ["launch"],
    priority: "high",
    status: "Inbox",
    scheduledDate: "2026-05-08",
    dueDate: "2026-05-10",
    estimateMinutes: 75,
    progressMode: "focus_time",
    progressValue: null,
    createdBy: "user-1",
    ...overrides,
  };
}

function row(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    id: "item-1",
    workspace_id: "workspace-1",
    board_id: "board-1",
    column_id: "inbox",
    goal_id: null,
    title: "Plan launch",
    description: "Draft the rollout",
    type: "content",
    tags: ["launch"],
    priority: "high",
    status: "Inbox",
    scheduled_date: "2026-05-08",
    due_date: "2026-05-10",
    estimate_minutes: 75,
    focus_minutes_total: 0,
    progress_mode: "focus_time",
    progress_value: null,
    sort_order: "2",
    ...overrides,
  };
}

function focusInput(overrides: Partial<Parameters<typeof logFocusSession>[0]> = {}) {
  return {
    workspaceId: "workspace-1",
    itemId: "item-1",
    goalId: "goal-1",
    startedAt: "2026-05-06T09:30:00.000Z",
    endedAt: "2026-05-06T10:00:00.000Z",
    durationMinutes: 30,
    ...overrides,
  };
}

interface ChainOptions {
  insert?: ReturnType<typeof vi.fn>;
  update?: ReturnType<typeof vi.fn>;
  eq?: ReturnType<typeof vi.fn>;
  single: unknown;
}

function chain(options: ChainOptions): never {
  const builder = {
    insert: options.insert ?? vi.fn(),
    update: options.update ?? vi.fn(),
    eq: options.eq ?? vi.fn(),
    select: vi.fn(),
    single: vi.fn(() => Promise.resolve(options.single)),
  };

  builder.insert.mockReturnValue(builder);
  builder.update.mockReturnValue(builder);
  builder.eq.mockReturnValue(builder);
  builder.select.mockReturnValue(builder);

  return builder as never;
}
