import { describe, expect, it } from "vitest";
import { calculateGoalProgress, calculateItemProgress } from "./progress";
import type { GoalProgressInput, ItemProgressInput } from "./types";

describe("calculateItemProgress", () => {
  it("uses manual progress when mode is manual", () => {
    const input: ItemProgressInput = { mode: "manual", progressValue: 42 };
    expect(calculateItemProgress(input)).toBe(42);
  });

  it("uses focus minutes against estimate for focus_time mode", () => {
    const input: ItemProgressInput = {
      mode: "focus_time",
      focusMinutesTotal: 45,
      estimateMinutes: 60,
    };
    expect(calculateItemProgress(input)).toBe(75);
  });

  it("caps item progress at 100", () => {
    const input: ItemProgressInput = {
      mode: "focus_time",
      focusMinutesTotal: 150,
      estimateMinutes: 60,
    };
    expect(calculateItemProgress(input)).toBe(100);
  });

  it("returns 100 for completed task mode and 0 otherwise", () => {
    expect(calculateItemProgress({ mode: "tasks", status: "Done" })).toBe(100);
    expect(calculateItemProgress({ mode: "tasks", status: "Doing" })).toBe(0);
  });
});

describe("calculateGoalProgress", () => {
  it("uses completed milestones for milestone mode", () => {
    const input: GoalProgressInput = {
      mode: "milestones",
      milestones: [
        { status: "done" },
        { status: "active" },
        { status: "done" },
      ],
      linkedItems: [],
      focusMinutesTotal: 0,
      focusTargetMinutes: 0,
      manualProgressValue: null,
    };

    expect(calculateGoalProgress(input)).toBe(67);
  });

  it("uses linked item completion for task mode", () => {
    const input: GoalProgressInput = {
      mode: "tasks",
      milestones: [],
      linkedItems: [{ status: "Done" }, { status: "Doing" }, { status: "Done" }],
      focusMinutesTotal: 0,
      focusTargetMinutes: 0,
      manualProgressValue: null,
    };

    expect(calculateGoalProgress(input)).toBe(67);
  });

  it("uses manual value for manual mode", () => {
    const input: GoalProgressInput = {
      mode: "manual",
      milestones: [],
      linkedItems: [],
      focusMinutesTotal: 0,
      focusTargetMinutes: 0,
      manualProgressValue: 88,
    };

    expect(calculateGoalProgress(input)).toBe(88);
  });
});
