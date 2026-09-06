import { describe, expect, it } from "vitest";
import type { WorkspaceItem } from "../../domain/types";
import { splitTodayItems } from "./todayUtils";

describe("splitTodayItems", () => {
  it("separates scheduled today and overdue unfinished items", () => {
    const result = splitTodayItems(
      [
        item("today", "2026-05-05", null, "Planned"),
        item("late", null, "2026-05-04", "Doing"),
        item("future", null, "2026-05-06", "Planned"),
        item("done", "2026-05-05", "2026-05-04", "Done"),
      ],
      "2026-05-05",
    );

    expect(result.today.map((entry) => entry.id)).toEqual(["today"]);
    expect(result.overdue.map((entry) => entry.id)).toEqual(["late"]);
  });
});

function item(
  id: string,
  scheduledDate: string | null,
  dueDate: string | null,
  status: WorkspaceItem["status"],
): WorkspaceItem {
  return {
    id,
    workspaceId: "workspace",
    boardId: "board",
    columnId: "column",
    goalId: null,
    title: id,
    description: null,
    type: "task",
    tags: [],
    priority: "medium",
    status,
    scheduledDate,
    dueDate,
    estimateMinutes: null,
    focusMinutesTotal: 0,
    progressMode: "tasks",
    progressValue: null,
    sortOrder: 0,
  };
}
