import { describe, expect, it } from "vitest";
import { summarizeGoal } from "./goalUtils";

describe("summarizeGoal", () => {
  it("returns progress label and status label", () => {
    expect(summarizeGoal({ progress: 72, status: "active" })).toEqual({
      progressLabel: "72%",
      statusLabel: "Active",
    });
  });
});
