import { describe, expect, it } from "vitest";
import { parseEstimateTimeInput } from "./itemFormUtils";

describe("parseEstimateTimeInput", () => {
  it("returns null minutes for empty input", () => {
    expect(parseEstimateTimeInput("")).toEqual({ minutes: null, error: null });
    expect(parseEstimateTimeInput("   ")).toEqual({ minutes: null, error: null });
  });

  it("parses raw minutes and minute suffixes", () => {
    expect(parseEstimateTimeInput("45")).toEqual({ minutes: 45, error: null });
    expect(parseEstimateTimeInput("30 mins")).toEqual({ minutes: 30, error: null });
    expect(parseEstimateTimeInput("30 min")).toEqual({ minutes: 30, error: null });
    expect(parseEstimateTimeInput("30m")).toEqual({ minutes: 30, error: null });
  });

  it("parses hour suffixes and decimal hours", () => {
    expect(parseEstimateTimeInput("1 hour")).toEqual({ minutes: 60, error: null });
    expect(parseEstimateTimeInput("2 hours")).toEqual({ minutes: 120, error: null });
    expect(parseEstimateTimeInput("1h")).toEqual({ minutes: 60, error: null });
    expect(parseEstimateTimeInput("1.5 hrs")).toEqual({ minutes: 90, error: null });
  });

  it("rejects invalid estimates", () => {
    expect(parseEstimateTimeInput("abc")).toEqual({
      minutes: null,
      error: "Use minutes or hours, for example 30 mins or 1.5h.",
    });
    expect(parseEstimateTimeInput("-10")).toEqual({
      minutes: null,
      error: "Use minutes or hours, for example 30 mins or 1.5h.",
    });
  });
});
