import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";

vi.mock("./features/auth/AuthProvider", () => ({
  AuthProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  useAuth: () => ({
    loading: false,
    session: null,
    user: { id: "user-1" },
  }),
}));

vi.mock("./features/workspaces/useWorkspace", () => ({
  useWorkspace: () => ({
    data: { id: "workspace-1", name: "Personal Workspace", owner_user_id: "user-1" },
    isError: false,
    isLoading: false,
  }),
}));

vi.mock("./features/boards/BoardPage", () => ({
  BoardPage: () => <section>Board view</section>,
}));

vi.mock("./features/dashboard/DashboardPage", () => ({
  DashboardPage: ({
    onOpenBoard,
    onOpenGoals,
  }: {
    onOpenBoard: () => void;
    onOpenGoals: () => void;
  }) => (
    <section>
      Dashboard view
      <button type="button" onClick={onOpenBoard}>
        Dashboard board shortcut
      </button>
      <button type="button" onClick={onOpenGoals}>
        Dashboard goals shortcut
      </button>
    </section>
  ),
}));

vi.mock("./features/goals/GoalsPage", () => ({
  GoalsPage: () => <section>Goals view</section>,
}));

describe("App navigation", () => {
  it("starts on dashboard and switches between dashboard, board, and goals views", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>,
    );

    expect(screen.getByText("Dashboard view")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Board" }));
    expect(screen.getByText("Board view")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Goals" }));
    expect(screen.getByText("Goals view")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Dashboard" }));
    expect(screen.getByText("Dashboard view")).toBeVisible();
  });

  it("switches views from dashboard shortcuts", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Dashboard board shortcut" }));
    expect(screen.getByText("Board view")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Dashboard" }));
    await userEvent.click(screen.getByRole("button", { name: "Dashboard goals shortcut" }));
    expect(screen.getByText("Goals view")).toBeVisible();
  });

  it("switches views from command palette actions", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>,
    );

    fireEvent.keyDown(window, { ctrlKey: true, key: "k" });
    await userEvent.click(await screen.findByText("Open goals"));

    expect(screen.getByText("Goals view")).toBeVisible();
  });

  it("returns to dashboard from the command palette", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>,
    );

    await userEvent.click(screen.getByRole("button", { name: "Board" }));
    expect(screen.getByText("Board view")).toBeVisible();

    fireEvent.keyDown(window, { ctrlKey: true, key: "k" });
    await userEvent.click(await screen.findByText("Open dashboard"));

    expect(screen.getByText("Dashboard view")).toBeVisible();
  });
});
