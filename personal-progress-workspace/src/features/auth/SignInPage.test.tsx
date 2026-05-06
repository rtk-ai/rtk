import "@testing-library/jest-dom/vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { SignInPage } from "./SignInPage";

vi.mock("../../lib/env", () => ({
  env: {
    supabaseUrl: "",
    supabaseAnonKey: "",
    hasSupabaseConfig: false,
  },
}));

vi.mock("../../lib/supabase", () => ({
  supabase: {
    auth: {
      signInWithPassword: vi.fn(),
    },
  },
}));

describe("SignInPage", () => {
  it("does not attempt sign-in when Supabase config is missing", async () => {
    render(<SignInPage />);

    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByText("Supabase configuration is missing.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Sign in" })).toBeDisabled();
    expect(supabase.auth.signInWithPassword).not.toHaveBeenCalled();
  });
});
