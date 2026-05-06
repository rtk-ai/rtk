import { describe, expect, it } from "vitest";
import { commandActions } from "./commandActions";

describe("commandActions", () => {
  it("contains the primary workspace commands", () => {
    expect(commandActions.map((action) => action.id)).toEqual([
      "create-item",
      "open-board",
      "open-goals",
      "log-focus",
    ]);
  });
});
