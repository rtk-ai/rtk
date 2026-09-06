import { describe, expect, it } from "vitest";
import type { BoardData } from "../boards/boardApi";
import type { GoalBundle } from "../goals/goalApi";
import { buildDashboardSummary } from "./dashboardUtils";

const board: BoardData = {
  boardId: "board-1",
  columns: [
    { id: "column-1", boardId: "board-1", name: "Inbox", statusKey: "Inbox", sortOrder: 1 },
    { id: "column-2", boardId: "board-1", name: "Doing", statusKey: "Doing", sortOrder: 2 },
    { id: "column-3", boardId: "board-1", name: "Done", statusKey: "Done", sortOrder: 3 },
  ],
  items: [
    {
      id: "item-1",
      workspaceId: "workspace-1",
      boardId: "board-1",
      columnId: "column-2",
      goalId: "goal-1",
      title: "Ship dashboard",
      description: null,
      type: "task",
      tags: ["app"],
      priority: "urgent",
      status: "Doing",
      scheduledDate: "2026-05-08",
      dueDate: "2026-05-08",
      estimateMinutes: 120,
      focusMinutesTotal: 45,
      progressMode: "focus_time",
      progressValue: null,
      sortOrder: 1,
    },
    {
      id: "item-2",
      workspaceId: "workspace-1",
      boardId: "board-1",
      columnId: "column-1",
      goalId: "goal-1",
      title: "Write content brief",
      description: null,
      type: "content",
      tags: [],
      priority: "high",
      status: "Planned",
      scheduledDate: null,
      dueDate: "2026-05-06",
      estimateMinutes: 60,
      focusMinutesTotal: 15,
      progressMode: "tasks",
      progressValue: null,
      sortOrder: 2,
    },
    {
      id: "item-3",
      workspaceId: "workspace-1",
      boardId: "board-1",
      columnId: "column-3",
      goalId: "goal-2",
      title: "Archive finished notes",
      description: null,
      type: "personal",
      tags: [],
      priority: "low",
      status: "Done",
      scheduledDate: "2026-05-08",
      dueDate: "2026-05-07",
      estimateMinutes: 30,
      focusMinutesTotal: 30,
      progressMode: "tasks",
      progressValue: null,
      sortOrder: 3,
    },
  ],
};

const goals: GoalBundle = {
  goals: [
    {
      id: "goal-1",
      workspaceId: "workspace-1",
      title: "Launch personal OS",
      description: null,
      status: "active",
      targetDate: "2026-05-12",
      progressMode: "tasks",
      manualProgressValue: null,
    },
    {
      id: "goal-2",
      workspaceId: "workspace-1",
      title: "Clean knowledge base",
      description: null,
      status: "active",
      targetDate: "2026-05-20",
      progressMode: "manual",
      manualProgressValue: 80,
    },
  ],
  milestones: [
    { id: "milestone-1", goalId: "goal-1", title: "Dashboard", status: "active", dueDate: "2026-05-08", sortOrder: 1 },
    { id: "milestone-2", goalId: "goal-2", title: "Archive", status: "done", dueDate: "2026-05-09", sortOrder: 1 },
  ],
  linkedItems: board.items.filter((item) => item.goalId),
};

describe("buildDashboardSummary", () => {
  it("summarizes board health, focus, today, and priority items", () => {
    const summary = buildDashboardSummary({ board, goals, today: "2026-05-08" });

    expect(summary.stats).toMatchObject({
      totalItems: 3,
      openItems: 2,
      doneItems: 1,
      doingItems: 1,
      todayItems: 1,
      overdueItems: 1,
      urgentItems: 1,
      focusMinutes: 90,
      focusHoursLabel: "1.5h",
    });
    expect(summary.today.scheduled.map((item) => item.title)).toEqual(["Ship dashboard"]);
    expect(summary.today.overdue.map((item) => item.title)).toEqual(["Write content brief"]);
    expect(summary.priorityItems.map((item) => item.title)).toEqual(["Write content brief", "Ship dashboard"]);
    expect(summary.focus.completionPercent).toBe(43);
  });

  it("summarizes goal progress with linked item counts", () => {
    const summary = buildDashboardSummary({ board, goals, today: "2026-05-08" });

    expect(summary.goalHighlights).toEqual([
      expect.objectContaining({
        id: "goal-1",
        title: "Launch personal OS",
        progress: 0,
        progressLabel: "0%",
        openItems: 2,
        overdueItems: 1,
      }),
      expect.objectContaining({
        id: "goal-2",
        title: "Clean knowledge base",
        progress: 80,
        progressLabel: "80%",
        openItems: 0,
        overdueItems: 0,
      }),
    ]);
  });
});
