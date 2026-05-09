import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("App", () => {
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
});
