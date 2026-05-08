import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardData } from "./dashboardApi";
import { loadDashboard } from "./dashboardApi";
import { DashboardPage } from "./DashboardPage";

vi.mock("./dashboardApi", () => ({
  loadDashboard: vi.fn(),
}));

function renderDashboard(data: DashboardData) {
  vi.mocked(loadDashboard).mockResolvedValue(data);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const onOpenBoard = vi.fn();
  const onOpenGoals = vi.fn();

  render(
    <QueryClientProvider client={queryClient}>
      <DashboardPage
        workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }}
        onOpenBoard={onOpenBoard}
        onOpenGoals={onOpenGoals}
      />
    </QueryClientProvider>,
  );

  return { onOpenBoard, onOpenGoals };
}

const dashboardData: DashboardData = {
  board: {
    boardId: "board-1",
    columns: [],
    items: [
      {
        id: "item-1",
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "column-1",
        goalId: "goal-1",
        title: "Ship dashboard",
        description: null,
        type: "task",
        tags: [],
        priority: "urgent",
        status: "Doing",
        scheduledDate: new Date().toISOString().slice(0, 10),
        dueDate: new Date().toISOString().slice(0, 10),
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
        title: "Review weekly notes",
        description: null,
        type: "learning",
        tags: [],
        priority: "high",
        status: "Planned",
        scheduledDate: null,
        dueDate: "2020-01-01",
        estimateMinutes: 60,
        focusMinutesTotal: 0,
        progressMode: "tasks",
        progressValue: null,
        sortOrder: 2,
      },
    ],
  },
  goals: {
    goals: [
      {
        id: "goal-1",
        workspaceId: "workspace-1",
        title: "Launch personal OS",
        description: null,
        status: "active",
        targetDate: "2026-05-20",
        progressMode: "tasks",
        manualProgressValue: null,
      },
    ],
    milestones: [],
    linkedItems: [],
  },
};

describe("DashboardPage", () => {
  beforeEach(() => {
    vi.mocked(loadDashboard).mockReset();
  });

  it("renders a dashboard overview from workspace data", async () => {
    renderDashboard(dashboardData);

    expect(await screen.findByRole("heading", { name: "Command dashboard" })).toBeVisible();
    expect(screen.getByText("2 open")).toBeVisible();
    expect(screen.getByText("1 due now")).toBeVisible();
    expect(screen.getAllByText("Ship dashboard")).toHaveLength(2);
    expect(screen.getByText("Launch personal OS")).toBeVisible();
  });

  it("exposes quick actions for board and goal views", async () => {
    const { onOpenBoard, onOpenGoals } = renderDashboard(dashboardData);

    await userEvent.click(await screen.findByRole("button", { name: "Open board" }));
    await userEvent.click(screen.getByRole("button", { name: "Review goals" }));

    expect(onOpenBoard).toHaveBeenCalledTimes(1);
    expect(onOpenGoals).toHaveBeenCalledTimes(1);
  });
});
