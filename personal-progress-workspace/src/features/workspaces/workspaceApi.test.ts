import { beforeEach, describe, expect, it, vi } from "vitest";
import { supabase } from "../../lib/supabase";
import { getOrCreatePersonalWorkspace } from "./workspaceApi";

vi.mock("../../lib/supabase", () => ({
  supabase: {
    from: vi.fn(),
  },
}));

describe("getOrCreatePersonalWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("repairs missing default columns before returning an existing workspace", async () => {
    const insertColumns = vi.fn().mockResolvedValue({ error: null });
    const from = vi.mocked(supabase.from);

    from.mockImplementation((table: string) => {
      if (table === "workspace_members") {
        return chain({
          maybeSingle: { data: { workspaces: { id: "workspace-1", name: "Personal", owner_user_id: "user-1" } }, error: null },
        });
      }

      if (table === "boards") {
        return chain({
          maybeSingle: { data: { id: "board-1" }, error: null },
        });
      }

      if (table === "board_columns") {
        return {
          select: vi.fn(() => ({
            eq: vi.fn(() => Promise.resolve({ data: [{ status_key: "Inbox" }, { status_key: "Done" }], error: null })),
          })),
          insert: insertColumns,
        } as never;
      }

      throw new Error(`Unexpected table: ${table}`);
    });

    await getOrCreatePersonalWorkspace(user("user-1"));

    expect(insertColumns).toHaveBeenCalledWith([
      { board_id: "board-1", name: "Planned", status_key: "Planned", sort_order: 1 },
      { board_id: "board-1", name: "Doing", status_key: "Doing", sort_order: 2 },
      { board_id: "board-1", name: "Review", status_key: "Review", sort_order: 3 },
    ]);
  });

  it("repairs an owned workspace when owner membership was not created", async () => {
    const membershipUpsert = vi.fn().mockResolvedValue({ error: null });
    const insertColumns = vi.fn().mockResolvedValue({ error: null });
    const ownedWorkspace = { id: "workspace-owned", name: "Personal Workspace", owner_user_id: "user-1" };
    const from = vi.mocked(supabase.from);

    from.mockImplementation((table: string) => {
      if (table === "workspace_members") {
        return {
          select: vi.fn(() => ({
            eq: vi.fn(() => ({
              limit: vi.fn(() => ({
                maybeSingle: vi.fn(() => Promise.resolve({ data: null, error: null })),
              })),
            })),
          })),
          upsert: membershipUpsert,
        } as never;
      }

      if (table === "workspaces") {
        return chain({
          maybeSingle: { data: ownedWorkspace, error: null },
        });
      }

      if (table === "boards") {
        return chain({
          maybeSingle: { data: { id: "board-owned" }, error: null },
        });
      }

      if (table === "board_columns") {
        return {
          select: vi.fn(() => ({
            eq: vi.fn(() => Promise.resolve({ data: [{ status_key: "Inbox" }], error: null })),
          })),
          insert: insertColumns,
        } as never;
      }

      throw new Error(`Unexpected table: ${table}`);
    });

    await expect(getOrCreatePersonalWorkspace(user("user-1"))).resolves.toEqual(ownedWorkspace);
    expect(membershipUpsert).toHaveBeenCalledWith(
      { workspace_id: "workspace-owned", user_id: "user-1", role: "owner" },
      { ignoreDuplicates: true, onConflict: "workspace_id,user_id" },
    );
    expect(insertColumns).toHaveBeenCalledWith([
      { board_id: "board-owned", name: "Planned", status_key: "Planned", sort_order: 1 },
      { board_id: "board-owned", name: "Doing", status_key: "Doing", sort_order: 2 },
      { board_id: "board-owned", name: "Review", status_key: "Review", sort_order: 3 },
      { board_id: "board-owned", name: "Done", status_key: "Done", sort_order: 4 },
    ]);
  });
});

function user(id: string) {
  return { id } as Parameters<typeof getOrCreatePersonalWorkspace>[0];
}

interface MockBuilder {
  select: ReturnType<typeof vi.fn>;
  eq: ReturnType<typeof vi.fn>;
  order: ReturnType<typeof vi.fn>;
  limit: ReturnType<typeof vi.fn>;
  maybeSingle: ReturnType<typeof vi.fn>;
  single: ReturnType<typeof vi.fn>;
  insert: ReturnType<typeof vi.fn>;
}

function chain(results: {
  maybeSingle?: unknown;
  single?: unknown;
  select?: unknown;
  insert?: ReturnType<typeof vi.fn>;
}): never {
  const builder: MockBuilder = {
    select: vi.fn((): unknown => results.select ?? builder),
    eq: vi.fn((): MockBuilder => builder),
    order: vi.fn((): MockBuilder => builder),
    limit: vi.fn((): MockBuilder => builder),
    maybeSingle: vi.fn(() => Promise.resolve(results.maybeSingle)),
    single: vi.fn(() => Promise.resolve(results.single)),
    insert: results.insert ?? vi.fn((): MockBuilder => builder),
  };

  return builder as never;
}
