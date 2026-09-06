# Item And Goal Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make board item scheduling, estimates, and status easier to edit manually, and make the Goals tab support creating and editing goals.

**Architecture:** Keep existing Supabase schema and mutation contracts. Add a small estimate parser for item form normalization, pass board columns into the form so status choices can update the matching column, then add focused goal create/update API functions and local Goals page form state.

**Tech Stack:** React 19, TypeScript, TanStack Query, Supabase JS, Vitest, Testing Library, Playwright, Vite.

---

### Task 1: Item Estimate Parser

**Files:**
- Create: `personal-progress-workspace/src/features/boards/itemFormUtils.ts`
- Create: `personal-progress-workspace/src/features/boards/itemFormUtils.test.ts`

- [ ] **Step 1: Write failing parser tests**

Add tests for empty input, raw minutes, minute suffixes, hour suffixes, decimal hours, and invalid text:

```ts
expect(parseEstimateTimeInput("")).toEqual({ minutes: null, error: null });
expect(parseEstimateTimeInput("45")).toEqual({ minutes: 45, error: null });
expect(parseEstimateTimeInput("30 mins")).toEqual({ minutes: 30, error: null });
expect(parseEstimateTimeInput("1h")).toEqual({ minutes: 60, error: null });
expect(parseEstimateTimeInput("1.5 hours")).toEqual({ minutes: 90, error: null });
expect(parseEstimateTimeInput("abc").error).toBe("Use minutes or hours, for example 30 mins or 1.5h.");
```

- [ ] **Step 2: Verify parser tests fail**

Run: `npm test -- itemFormUtils.test.ts`

Expected: FAIL because `itemFormUtils.ts` does not exist yet.

- [ ] **Step 3: Implement parser**

Create `parseEstimateTimeInput(value)` returning `{ minutes, error }`. Trim input, accept positive numeric minutes, `m/min/mins/minute/minutes`, `h/hr/hrs/hour/hours`, round decimal hours to whole minutes, and reject negative or unrecognized values.

- [ ] **Step 4: Verify parser tests pass**

Run: `npm test -- itemFormUtils.test.ts`

Expected: PASS.

### Task 2: Item Form Manual Editing

**Files:**
- Modify: `personal-progress-workspace/src/features/boards/ItemForm.tsx`
- Modify: `personal-progress-workspace/src/features/boards/ItemDrawer.tsx`
- Modify: `personal-progress-workspace/src/features/boards/ItemForm.test.tsx`

- [ ] **Step 1: Write failing form tests**

Update tests so `Scheduled date` and `Due date` are text inputs, `Estimate time` accepts manual strings and presets, and the visible progress control maps labels to item statuses:

```ts
await userEvent.selectOptions(screen.getByLabelText("Progress status"), "Doing");
expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
  status: "Doing",
  columnId: "doing",
  estimateMinutes: 90,
  progressMode: "manual",
}));
```

Also add an invalid estimate test that submits `abc`, expects the inline error, and expects `onSubmit` not to be called.

- [ ] **Step 2: Verify form tests fail**

Run: `npm test -- ItemForm.test.tsx`

Expected: FAIL because the old form still exposes `Estimate minutes` and `Progress mode`.

- [ ] **Step 3: Implement form changes**

Add `columns` prop to `ItemForm`. Replace date inputs with text inputs using `placeholder="YYYY-MM-DD"`. Rename label to `Estimate time`, add preset buttons for `30 mins`, `1 hour`, `2 hours`, and `4 hours`, parse on submit, and replace the progress mode select with `Progress status` options:

```ts
const statusOptions = [
  { value: "Planned", label: "Chưa hoàn thành" },
  { value: "Doing", label: "Đang làm" },
  { value: "Done", label: "Đã hoàn thành" },
] as const;
```

When status changes, find `columns.find((column) => column.statusKey === progressStatus)` and submit its id as `columnId`. Keep existing `progressMode` for existing items and use `manual` for new items.

- [ ] **Step 4: Pass columns from drawer**

Update `ItemDrawer` to pass `columns={board.columns}` into `ItemForm`.

- [ ] **Step 5: Verify form tests pass**

Run: `npm test -- ItemForm.test.tsx itemFormUtils.test.ts`

Expected: PASS.

### Task 3: Goal API Mutations

**Files:**
- Modify: `personal-progress-workspace/src/features/goals/goalApi.ts`
- Modify: `personal-progress-workspace/src/features/goals/goalApi.test.ts`

- [ ] **Step 1: Write failing API tests**

Add tests for `createGoal` inserting:

```ts
{
  workspace_id: "workspace-1",
  title: "Build portfolio",
  description: "Ship the first version",
  status: "active",
  target_date: "2026-06-20",
  progress_mode: "manual",
  manual_progress_value: 0,
}
```

Add a test for `updateGoal("goal-1", input)` updating the editable fields while scoping by `id` and `workspace_id`.

- [ ] **Step 2: Verify API tests fail**

Run: `npm test -- goalApi.test.ts`

Expected: FAIL because `createGoal` and `updateGoal` do not exist.

- [ ] **Step 3: Implement API functions**

Export `GoalInput`, `createGoal(input)`, `updateGoal(goalId, input)`, and reuse `mapGoal` for returned rows. Use `.insert(...).select(...).single()` and `.update(...).eq("id", goalId).eq("workspace_id", input.workspaceId).select(...).single()`.

- [ ] **Step 4: Verify API tests pass**

Run: `npm test -- goalApi.test.ts`

Expected: PASS.

### Task 4: Editable Goals Page

**Files:**
- Modify: `personal-progress-workspace/src/features/goals/GoalsPage.tsx`
- Modify: `personal-progress-workspace/src/features/goals/GoalsPage.test.tsx`
- Modify: `personal-progress-workspace/src/styles/app.css`

- [ ] **Step 1: Write failing Goals page tests**

Mock `createGoal` and `updateGoal`. Add one test that fills the create form and expects:

```ts
expect(createGoal).toHaveBeenCalledWith({
  workspaceId: "workspace-1",
  title: "Build portfolio",
  description: "Ship the first version",
  status: "active",
  targetDate: "2026-06-20",
});
```

Add one test that clicks `Edit goal`, changes title/status/date, clicks `Save goal`, and expects `updateGoal` to receive the edited values.

- [ ] **Step 2: Verify Goals page tests fail**

Run: `npm test -- GoalsPage.test.tsx`

Expected: FAIL because the page has no create or edit controls.

- [ ] **Step 3: Implement create form and mutations**

Use `useMutation` and `useQueryClient`. Add a compact create form above the grid with Title, Description, Target date, Status, and `Create goal`. On success, invalidate `["goals", workspace.id]` and clear the form.

- [ ] **Step 4: Implement inline edit form**

Each goal card gets an `Edit goal` button. Edit mode shows controls for title, description, target date, and status plus `Save goal` and `Cancel edit`. Save calls `updateGoal(goal.id, input)`, invalidates the goals query, and exits edit mode.

- [ ] **Step 5: Style new controls**

Add CSS for `.goal-compose`, `.goal-form`, `.goal-form__grid`, `.goal-card__actions`, `.goal-card__button`, and `.goal-edit-form`. Reuse existing dark input styling and 8px radii.

- [ ] **Step 6: Verify Goals page tests pass**

Run: `npm test -- GoalsPage.test.tsx goalApi.test.ts`

Expected: PASS.

### Task 5: Full Verification And Deploy

**Files:**
- No source changes unless verification exposes a real issue.

- [ ] **Step 1: Run focused tests**

Run: `npm test -- ItemForm.test.tsx itemFormUtils.test.ts goalApi.test.ts GoalsPage.test.tsx`

Expected: PASS.

- [ ] **Step 2: Run full test suite**

Run: `npm test`

Expected: PASS.

- [ ] **Step 3: Build**

Run: `npm run build`

Expected: PASS with Vite production output.

- [ ] **Step 4: Run e2e**

Run: `npm run e2e`

Expected: PASS.

- [ ] **Step 5: Commit source branch**

Commit implementation with message `feat: add manual item and goal editing`.

- [ ] **Step 6: Update PR branch and deploy**

Cherry-pick the implementation commit to `codex/personal-progress-workspace-webapp`, push `fork`, deploy production with Vercel, and smoke-test `https://personal-progress-workspace.vercel.app`.

### Self-Review

- Spec coverage: Item manual date edits, estimate time presets/manual parsing, Vietnamese status labels, goal create, and inline goal edit all have implementation and test tasks.
- Placeholder scan: No TBD/TODO/later placeholders.
- Type consistency: `ItemInput`, `GoalInput`, `ItemStatus`, and `GoalStatus` names match existing domain/API files.
