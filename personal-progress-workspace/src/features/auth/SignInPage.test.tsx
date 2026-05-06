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

  it("does not attempt sign-in when Supabase config is missing", async () => {
    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByText("Supabase configuration is missing.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Sign in" })).toBeDisabled();
    expect(supabase.auth.signInWithPassword).not.toHaveBeenCalled();
  });

  it("signs in with an existing account when configured", async () => {
    authMocks.hasSupabaseConfig = true;
    authMocks.signInWithPassword.mockResolvedValue({ error: null });

    render(<SignInPage />);

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

    await userEvent.click(screen.getByRole("button", { name: "Create account instead" }));
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
