import { expect, test } from "@playwright/test";

test("shows sign in screen", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Create account" })).toBeVisible();
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page.getByRole("heading", { name: /sign in to your command center/i })).toBeVisible();
});
