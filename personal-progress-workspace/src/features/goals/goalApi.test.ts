import { beforeEach, describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { loadGoals } from "./goalApi";

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

function chain(result: { data: unknown; error: unknown }): never {
  const builder = {
    select: vi.fn(() => builder),
    eq: vi.fn(() => builder),
    order: vi.fn(() => Promise.resolve(result)),
    not: vi.fn(() => Promise.resolve(result)),
  };

  return builder as never;
}
