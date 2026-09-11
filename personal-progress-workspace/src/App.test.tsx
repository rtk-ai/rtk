import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";

const appMocks = vi.hoisted(() => ({
  authLoading: false,
  user: null as { id: string } | null,
  workspace: {
    data: null as unknown,
    error: null as unknown,
    isError: false,
    isLoading: false,
    refetch: vi.fn(),
  },
}));

vi.mock("./features/auth/AuthProvider", () => ({
  AuthProvider: ({ children }: { children: ReactNode }) => children,
  useAuth: () => ({
    loading: appMocks.authLoading,
    session: appMocks.user ? { user: appMocks.user } : null,
    user: appMocks.user,
  }),
}));

vi.mock("./features/workspaces/useWorkspace", () => ({
  useWorkspace: () => appMocks.workspace,
}));

describe("App", () => {
  beforeEach(() => {
    appMocks.authLoading = false;
    appMocks.user = null;
    appMocks.workspace = {
      data: null,
      error: null,
      isError: false,
      isLoading: false,
      refetch: vi.fn(),
    };
  });

  it("renders the unauthenticated sign-in page", async () => {
    const testQueryClient = new QueryClient();

    render(
      <QueryClientProvider client={testQueryClient}>
        <App />
      </QueryClientProvider>,
    );

    expect(await screen.findByRole("button", { name: "Log in" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Sign in to your command center" })).not.toBeInTheDocument();
  });

  it("shows workspace load error details and allows retry", async () => {
    appMocks.user = { id: "user-1" };
    appMocks.workspace = {
      data: null,
      error: new Error("new row violates row-level security policy"),
      isError: true,
      isLoading: false,
      refetch: vi.fn(),
    };

    const testQueryClient = new QueryClient();

    render(
      <QueryClientProvider client={testQueryClient}>
        <App />
      </QueryClientProvider>,
    );

    expect(screen.getByRole("heading", { name: "Workspace failed to load" })).toBeVisible();
    expect(screen.getByText("new row violates row-level security policy")).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Retry workspace load" }));

    expect(appMocks.workspace.refetch).toHaveBeenCalled();
  });
});
