import "@testing-library/jest-dom/vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { SignInPage } from "./SignInPage";

const authMocks = vi.hoisted(() => ({
  hasSupabaseConfig: false,
  signInWithPassword: vi.fn(),
  signUp: vi.fn(),
}));

vi.mock("../../lib/env", () => ({
  env: {
    supabaseUrl: "https://project.supabase.co",
    supabaseAnonKey: "anon-key",
    get hasSupabaseConfig() {
      return authMocks.hasSupabaseConfig;
    },
  },
}));

vi.mock("../../lib/supabase", () => ({
  supabase: {
    auth: {
      signInWithPassword: authMocks.signInWithPassword,
      signUp: authMocks.signUp,
    },
  },
}));

describe("SignInPage", () => {
  beforeEach(() => {
    authMocks.hasSupabaseConfig = false;
    authMocks.signInWithPassword.mockReset();
    authMocks.signUp.mockReset();
  });

  it("keeps auth submit available when Supabase config is missing and explains setup", async () => {
    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Create account" }));
    await userEvent.type(screen.getByLabelText("Email"), "new@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "secure-password");

    expect(screen.queryByText("Supabase configuration is missing.")).not.toBeInTheDocument();
    expect(
      screen.getByText("Cloud sync is not connected yet. Add Supabase env vars in .env.local before accounts can be created."),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Create account" })).toBeEnabled();

    await userEvent.click(screen.getByRole("button", { name: "Create account" }));

    expect(
      screen.getByText(
        "Create account is not available until Supabase is connected. Add VITE_SUPABASE_URL and VITE_SUPABASE_ANON_KEY, then restart the dev server.",
      ),
    ).toBeVisible();
    expect(supabase.auth.signInWithPassword).not.toHaveBeenCalled();
    expect(supabase.auth.signUp).not.toHaveBeenCalled();
  });

  it("opens the auth form from a small top-right login button", async () => {
    render(<SignInPage />);

    expect(screen.getByRole("button", { name: "Log in" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Create account" })).toBeVisible();
    expect(screen.queryByLabelText("Email")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Log in" }));

    expect(screen.getByLabelText("Email")).toBeVisible();
    expect(screen.getByRole("heading", { name: "Sign in to your command center" })).toBeVisible();
  });

  it("opens create-account mode from the top-right create account button", async () => {
    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Create account" }));

    expect(screen.getByLabelText("Email")).toBeVisible();
    expect(screen.getByRole("heading", { name: "Create your command center" })).toBeVisible();
  });

  it("signs in with an existing account when configured", async () => {
    authMocks.hasSupabaseConfig = true;
    authMocks.signInWithPassword.mockResolvedValue({ error: null });

    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    await userEvent.type(screen.getByLabelText("Email"), "me@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "secure-password");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(supabase.auth.signInWithPassword).toHaveBeenCalledWith({
      email: "me@example.com",
      password: "secure-password",
    });
  });

  it("creates a new account from the same auth screen", async () => {
    authMocks.hasSupabaseConfig = true;
    authMocks.signUp.mockResolvedValue({ data: { session: null }, error: null });

    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Create account" }));
    await userEvent.type(screen.getByLabelText("Email"), "new@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "secure-password");
    await userEvent.click(screen.getByRole("button", { name: "Create account" }));

    expect(supabase.auth.signUp).toHaveBeenCalledWith({
      email: "new@example.com",
      password: "secure-password",
    });
    expect(supabase.auth.signInWithPassword).not.toHaveBeenCalled();
    expect(await screen.findByText("Account created. Check your email to confirm it, then sign in.")).toBeVisible();
  });
});
