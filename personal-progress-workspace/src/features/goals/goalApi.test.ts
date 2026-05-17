import { beforeEach, describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { createGoal, loadGoals, updateGoal } from "./goalApi";

vi.mock("../../lib/supabase", () => ({
  supabase: {
    from: vi.fn(),
  },
}));

describe("loadGoals", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads goals, milestones, and linked items for a workspace", async () => {
    vi.mocked(supabase.from).mockImplementation((table: string) => {
      if (table === "goals") {
        return chain({
          data: [
            {
              id: "goal-1",
              workspace_id: "workspace-1",
              title: "Launch channel",
              description: "Publish consistently",
              status: "active",
              target_date: "2026-06-01",
              progress_mode: "manual",
              manual_progress_value: 72,
            },
          ],
          error: null,
        });
      }

      if (table === "milestones") {
        return chain({
          data: [
            {
              id: "milestone-1",
              workspace_id: "workspace-1",
              goal_id: "goal-1",
              title: "First draft",
              status: "done",
              due_date: "2026-05-10",
              sort_order: "2",
            },
          ],
          error: null,
        });
      }

      if (table === "items") {
        return chain({
          data: [
            {
              id: "item-1",
              workspace_id: "workspace-1",
              board_id: "board-1",
              column_id: "doing",
              goal_id: "goal-1",
              title: "Write episode outline",
              description: null,
              type: "content",
              tags: null,
              priority: "high",
              status: "Doing",
              scheduled_date: "2026-05-08",
              due_date: "2026-05-12",
              estimate_minutes: 90,
              focus_minutes_total: 45,
              progress_mode: "focus_time",
              progress_value: null,
              sort_order: "3",
            },
          ],
          error: null,
        });
      }

      throw new Error(`Unexpected table: ${table}`);
    });

    await expect(loadGoals("workspace-1")).resolves.toEqual({
      goals: [
        {
          id: "goal-1",
          workspaceId: "workspace-1",
          title: "Launch channel",
          description: "Publish consistently",
          status: "active",
          targetDate: "2026-06-01",
          progressMode: "manual",
          manualProgressValue: 72,
        },
      ],
      milestones: [
        {
          id: "milestone-1",
          goalId: "goal-1",
          title: "First draft",
          status: "done",
          dueDate: "2026-05-10",
          sortOrder: 2,
        },
      ],
      linkedItems: [
        expect.objectContaining({
          id: "item-1",
          workspaceId: "workspace-1",
          goalId: "goal-1",
          tags: [],
          sortOrder: 3,
        }),
      ],
    });
  });
});

describe("goal mutations", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("creates a manual progress goal", async () => {
    const insert = vi.fn(() => ({
      select: vi.fn(() => ({
        single: vi.fn(() =>
          Promise.resolve({
            data: goalRow({ id: "goal-new", title: "Build portfolio" }),
            error: null,
          }),
        ),
      })),
    }));
    vi.mocked(supabase.from).mockReturnValue({ insert } as never);

    await expect(
      createGoal({
        workspaceId: "workspace-1",
        title: "Build portfolio",
        description: "Ship the first version",
        status: "active",
        targetDate: "2026-06-20",
      }),
    ).resolves.toEqual({
      id: "goal-new",
      workspaceId: "workspace-1",
      title: "Build portfolio",
      description: "Ship the first version",
      status: "active",
      targetDate: "2026-06-20",
      progressMode: "manual",
      manualProgressValue: 0,
    });

    expect(supabase.from).toHaveBeenCalledWith("goals");
    expect(insert).toHaveBeenCalledWith({
      workspace_id: "workspace-1",
      title: "Build portfolio",
      description: "Ship the first version",
      status: "active",
      target_date: "2026-06-20",
      progress_mode: "manual",
      manual_progress_value: 0,
    });
  });

  it("updates goal fields within the workspace", async () => {
    const single = vi.fn(() =>
      Promise.resolve({
        data: goalRow({
          title: "Launch portfolio",
          description: null,
          status: "paused",
          target_date: null,
        }),
        error: null,
      }),
    );
    const builder = {
      update: vi.fn(() => builder),
      eq: vi.fn(() => builder),
      select: vi.fn(() => ({ single })),
    };
    vi.mocked(supabase.from).mockReturnValue(builder as never);

    await expect(
      updateGoal("goal-1", {
        workspaceId: "workspace-1",
        title: "Launch portfolio",
        description: null,
        status: "paused",
        targetDate: null,
      }),
    ).resolves.toEqual({
      id: "goal-1",
      workspaceId: "workspace-1",
      title: "Launch portfolio",
      description: null,
      status: "paused",
      targetDate: null,
      progressMode: "manual",
      manualProgressValue: 0,
    });

    expect(builder.update).toHaveBeenCalledWith({
      title: "Launch portfolio",
      description: null,
      status: "paused",
      target_date: null,
    });
    expect(builder.eq).toHaveBeenNthCalledWith(1, "id", "goal-1");
    expect(builder.eq).toHaveBeenNthCalledWith(2, "workspace_id", "workspace-1");
  });
});

function goalRow(overrides: Record<string, unknown> = {}) {
  return {
    id: "goal-1",
    workspace_id: "workspace-1",
    title: "Build portfolio",
    description: "Ship the first version",
    status: "active",
    target_date: "2026-06-20",
    progress_mode: "manual",
    manual_progress_value: 0,
    ...overrides,
  };
}

function chain(result: { data: unknown; error: unknown }): never {
  const builder = {
    select: vi.fn(() => builder),
    eq: vi.fn(() => builder),
    order: vi.fn(() => Promise.resolve(result)),
    not: vi.fn(() => Promise.resolve(result)),
  };

  return builder as never;
}
