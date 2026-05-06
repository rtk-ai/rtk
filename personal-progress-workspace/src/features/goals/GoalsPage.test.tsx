import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { GoalBundle } from "./goalApi";
import { loadGoals } from "./goalApi";
import { GoalsPage } from "./GoalsPage";

vi.mock("./goalApi", async () => {
  const actual = await vi.importActual<typeof import("./goalApi")>("./goalApi");

  return {
    ...actual,
    loadGoals: vi.fn(),
  };
});

describe("GoalsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(loadGoals).mockResolvedValue(goalBundle());
  });

  it("renders goal progress summaries", async () => {
    renderWithQueryClient(
      <GoalsPage workspace={{ id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" }} />,
    );

    expect(await screen.findByRole("heading", { name: "Launch channel" })).toBeVisible();
    expect(screen.getByText("Active")).toBeVisible();
    expect(screen.getByText("72%")).toBeVisible();
    expect(screen.getByText("First draft")).toBeVisible();
    expect(screen.getByText("Write episode outline")).toBeVisible();
  });
});

function renderWithQueryClient(children: ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });

  return render(<QueryClientProvider client={queryClient}>{children}</QueryClientProvider>);
}

function goalBundle(): GoalBundle {
  return {
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
        sortOrder: 0,
      },
    ],
    linkedItems: [
      {
        id: "item-1",
        workspaceId: "workspace-1",
        boardId: "board-1",
        columnId: "doing",
        goalId: "goal-1",
        title: "Write episode outline",
        description: null,
        type: "content",
        tags: [],
        priority: "high",
        status: "Doing",
        scheduledDate: null,
        dueDate: null,
        estimateMinutes: 90,
        focusMinutesTotal: 45,
        progressMode: "focus_time",
        progressValue: null,
        sortOrder: 0,
      },
    ],
  };
}
