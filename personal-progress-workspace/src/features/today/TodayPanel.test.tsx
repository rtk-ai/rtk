import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { WorkspaceItem } from "../../domain/types";
import { TodayPanel } from "./TodayPanel";

describe("TodayPanel", () => {
  it("renders today, overdue, and focus logging controls", async () => {
    const onLogFocus = vi.fn();
    render(
      <TodayPanel
        items={[
          item("today", "Today task", "2026-05-05", null, "Planned"),
          item("late", "Late task", null, "2026-05-04", "Doing"),
          item("done", "Done task", "2026-05-05", "2026-05-04", "Done"),
        ]}
        today="2026-05-05"
        onLogFocus={onLogFocus}
      />,
    );

    expect(screen.getByRole("heading", { name: "1 planned" })).toBeVisible();
    const scheduledSection = screen.getByRole("heading", { name: "Scheduled" }).closest("section") as HTMLElement;
    const overdueSection = screen.getByRole("heading", { name: "Overdue" }).closest("section") as HTMLElement;

    expect(within(scheduledSection).getByText("Today task")).toBeVisible();
    expect(within(overdueSection).getByText("Late task")).toBeVisible();
    expect(screen.queryByText("Done task")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Log focus" }));

    expect(onLogFocus).toHaveBeenCalledWith(expect.objectContaining({ id: "today" }), 25);
  });
});

function item(
  id: string,
  title: string,
  scheduledDate: string | null,
  dueDate: string | null,
  status: WorkspaceItem["status"],
): WorkspaceItem {
  return {
    id,
    workspaceId: "workspace-1",
    boardId: "board-1",
    columnId: "column-1",
    goalId: null,
    title,
    description: null,
    type: "task",
    tags: [],
    priority: "medium",
    status,
    scheduledDate,
    dueDate,
    estimateMinutes: 25,
    focusMinutesTotal: 0,
    progressMode: "focus_time",
    progressValue: null,
    sortOrder: 0,
  };
}
