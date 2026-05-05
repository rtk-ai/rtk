# Personal Progress Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a browser-based, Supabase-synced personal progress workspace with a board-first workflow, Today panel, Goals dashboard, focus logging, and command palette.

**Architecture:** Create a standalone TypeScript React app under `personal-progress-workspace/`. Supabase owns auth and persistence; React Query owns client-side server state; feature folders isolate board, today, goals, auth, and command palette behavior.

**Tech Stack:** Vite React TypeScript, Supabase JS, Supabase Postgres/RLS, React Query, dnd-kit, cmdk, lucide-react, date-fns, Vitest, Testing Library, Playwright.

---

## Reference Docs

- Vite React setup: https://vite.dev/guide/
- Supabase JavaScript client and auth: https://supabase.com/docs/reference/javascript/introduction
- Supabase Row Level Security: https://supabase.com/docs/guides/database/postgres/row-level-security
- dnd-kit sortable: https://docs.dndkit.com/presets/sortable
- cmdk command menu: https://cmdk.paco.me/

## File Structure

Create a new app directory:

```text
personal-progress-workspace/
  .env.example
  index.html
  package.json
  playwright.config.ts
  tsconfig.json
  vite.config.ts
  vitest.setup.ts
  supabase/
    migrations/
      202605050001_initial_schema.sql
  src/
    App.tsx
    main.tsx
    styles/
      tokens.css
      app.css
    lib/
      env.ts
      supabase.ts
      queryClient.ts
    domain/
      types.ts
      progress.ts
      progress.test.ts
      dates.ts
    features/
      auth/
        AuthProvider.tsx
        SignInPage.tsx
      workspaces/
        workspaceApi.ts
        useWorkspace.ts
      boards/
        boardApi.ts
        boardUtils.ts
        boardUtils.test.ts
        BoardPage.tsx
        BoardColumn.tsx
        ItemCard.tsx
        ItemDrawer.tsx
        ItemForm.tsx
      today/
        todayUtils.ts
        todayUtils.test.ts
        TodayPanel.tsx
        FocusLogger.tsx
      goals/
        goalUtils.ts
        goalUtils.test.ts
        GoalsPage.tsx
        GoalDetail.tsx
      command/
        CommandPalette.tsx
        commandActions.ts
    test/
      render.tsx
  tests/
    smoke.spec.ts
```

Responsibilities:

- `domain/`: shared types and pure calculations.
- `lib/`: environment, Supabase client, query client.
- `features/auth/`: session state and sign-in UI.
- `features/workspaces/`: personal workspace bootstrap.
- `features/boards/`: board data access, drag/drop utilities, board UI, item editing.
- `features/today/`: scheduled-today view and focus logging.
- `features/goals/`: milestone and progress views.
- `features/command/`: global command palette and command routing.
- `supabase/migrations/`: schema, RLS, indexes, and triggers.
- `tests/`: browser smoke tests.

## Locked Decisions

- Use Vite instead of Next.js because MVP does not need server rendering.
- Use hosted Supabase via env vars first; local Supabase remains compatible through the same env contract.
- Store item tags as `text[]` on `items` for MVP.
- Use `@dnd-kit/core` and `@dnd-kit/sortable` for board drag/drop.
- Use `cmdk` for command palette accessibility and keyboard behavior.
- Keep notifications, PWA install, collaboration UI, and AI parsing outside MVP.

---

### Task 1: Scaffold The React App

**Files:**
- Create: `personal-progress-workspace/package.json`
- Create: `personal-progress-workspace/vite.config.ts`
- Create: `personal-progress-workspace/vitest.setup.ts`
- Create: `personal-progress-workspace/.env.example`
- Create: `personal-progress-workspace/src/main.tsx`
- Create: `personal-progress-workspace/src/App.tsx`
- Create: `personal-progress-workspace/src/styles/tokens.css`
- Create: `personal-progress-workspace/src/styles/app.css`

- [ ] **Step 1: Generate the Vite app**

Run from repo root:

```powershell
npm create vite@latest personal-progress-workspace -- --template react-ts
```

Expected: a new `personal-progress-workspace/` directory with React TypeScript starter files.

- [ ] **Step 2: Install runtime and test dependencies**

Run:

```powershell
cd personal-progress-workspace
npm install @supabase/supabase-js @tanstack/react-query @dnd-kit/core @dnd-kit/sortable @dnd-kit/utilities cmdk lucide-react date-fns clsx
npm install -D vitest @testing-library/react @testing-library/jest-dom @testing-library/user-event jsdom playwright @playwright/test
```

Expected: `package-lock.json` updates and npm exits with code 0.

- [ ] **Step 3: Replace scripts in `package.json`**

Set scripts to:

```json
{
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "test:watch": "vitest",
    "e2e": "playwright test"
  }
}
```

- [ ] **Step 4: Configure Vitest in `vite.config.ts`**

Use this file shape:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: "./vitest.setup.ts",
    globals: true,
  },
});
```

- [ ] **Step 5: Create `vitest.setup.ts`**

```ts
import "@testing-library/jest-dom/vitest";
```

- [ ] **Step 6: Create `.env.example`**

```text
VITE_SUPABASE_URL=https://your-project.supabase.co
VITE_SUPABASE_ANON_KEY=your-anon-key
```

- [ ] **Step 7: Replace `src/main.tsx`**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { App } from "./App";
import { queryClient } from "./lib/queryClient";
import "./styles/tokens.css";
import "./styles/app.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 8: Create temporary `src/App.tsx`**

```tsx
export function App() {
  return (
    <main className="app-shell">
      <section className="hero-panel">
        <p className="eyebrow">Personal Progress Workspace</p>
        <h1>Board-first command center</h1>
        <p>Scaffold ready. Auth, boards, today, goals, and command palette arrive in the next tasks.</p>
      </section>
    </main>
  );
}
```

- [ ] **Step 9: Create `src/styles/tokens.css`**

```css
:root {
  color-scheme: dark;
  --bg: #070b14;
  --panel: #0e1626;
  --panel-strong: #141f33;
  --line: #25324a;
  --text: #eef4ff;
  --muted: #9fb0c8;
  --blue: #38bdf8;
  --green: #34d399;
  --amber: #f59e0b;
  --rose: #fb7185;
  --violet: #a78bfa;
  --shadow: 0 24px 80px rgba(0, 0, 0, 0.35);
}
```

- [ ] **Step 10: Create `src/styles/app.css`**

```css
* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 320px;
  min-height: 100vh;
  background: var(--bg);
  color: var(--text);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}

button,
input,
textarea,
select {
  font: inherit;
}

.app-shell {
  min-height: 100vh;
  padding: 32px;
  background:
    linear-gradient(145deg, rgba(56, 189, 248, 0.16), transparent 34%),
    linear-gradient(40deg, rgba(52, 211, 153, 0.12), transparent 38%),
    var(--bg);
}

.hero-panel {
  max-width: 960px;
  margin: 0 auto;
  padding: 32px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: rgba(14, 22, 38, 0.86);
  box-shadow: var(--shadow);
}

.eyebrow {
  margin: 0 0 12px;
  color: var(--blue);
  font-size: 13px;
  font-weight: 700;
  text-transform: uppercase;
}
```

- [ ] **Step 11: Verify scaffold**

Run:

```powershell
npm run build
npm test
```

Expected: build succeeds; Vitest reports no test files or passes with exit code 0.

- [ ] **Step 12: Commit**

```powershell
git add personal-progress-workspace
git commit -m "feat: scaffold progress workspace app"
```

---

### Task 2: Add Domain Types And Progress Calculations

**Files:**
- Create: `personal-progress-workspace/src/domain/types.ts`
- Create: `personal-progress-workspace/src/domain/progress.ts`
- Create: `personal-progress-workspace/src/domain/progress.test.ts`

- [ ] **Step 1: Write failing progress tests**

Create `src/domain/progress.test.ts`:

```ts
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
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```powershell
npm test -- src/domain/progress.test.ts
```

Expected: FAIL because `progress.ts` and `types.ts` do not exist.

- [ ] **Step 3: Create `src/domain/types.ts`**

```ts
export type ItemType = "task" | "content" | "learning" | "habit" | "personal";
export type ItemStatus = "Inbox" | "Planned" | "Doing" | "Review" | "Done";
export type ItemPriority = "low" | "medium" | "high" | "urgent";
export type ProgressMode = "tasks" | "milestones" | "focus_time" | "streak" | "manual";
export type GoalStatus = "active" | "paused" | "completed";
export type MilestoneStatus = "active" | "done";

export interface BoardColumn {
  id: string;
  boardId: string;
  name: ItemStatus;
  statusKey: ItemStatus;
  sortOrder: number;
}

export interface WorkspaceItem {
  id: string;
  workspaceId: string;
  boardId: string;
  columnId: string;
  goalId: string | null;
  title: string;
  description: string | null;
  type: ItemType;
  tags: string[];
  priority: ItemPriority;
  status: ItemStatus;
  scheduledDate: string | null;
  dueDate: string | null;
  estimateMinutes: number | null;
  focusMinutesTotal: number;
  progressMode: ProgressMode;
  progressValue: number | null;
  sortOrder: number;
}

export interface Goal {
  id: string;
  workspaceId: string;
  title: string;
  description: string | null;
  status: GoalStatus;
  targetDate: string | null;
  progressMode: ProgressMode;
  manualProgressValue: number | null;
}

export interface Milestone {
  id: string;
  goalId: string;
  title: string;
  status: MilestoneStatus;
  dueDate: string | null;
  sortOrder: number;
}

export interface ItemProgressInput {
  mode: ProgressMode;
  status?: ItemStatus;
  progressValue?: number | null;
  estimateMinutes?: number | null;
  focusMinutesTotal?: number | null;
  streakCompletedToday?: boolean;
}

export interface GoalProgressInput {
  mode: ProgressMode;
  milestones: Array<{ status: MilestoneStatus }>;
  linkedItems: Array<{ status: ItemStatus }>;
  focusMinutesTotal: number;
  focusTargetMinutes: number;
  manualProgressValue: number | null;
}
```

- [ ] **Step 4: Create `src/domain/progress.ts`**

```ts
import type { GoalProgressInput, ItemProgressInput } from "./types";

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, Math.round(value)));
}

function ratioPercent(done: number, total: number): number {
  if (total <= 0) return 0;
  return clampPercent((done / total) * 100);
}

export function calculateItemProgress(input: ItemProgressInput): number {
  if (input.mode === "manual") {
    return clampPercent(input.progressValue ?? 0);
  }

  if (input.mode === "focus_time") {
    return ratioPercent(input.focusMinutesTotal ?? 0, input.estimateMinutes ?? 0);
  }

  if (input.mode === "streak") {
    return input.streakCompletedToday ? 100 : 0;
  }

  if (input.mode === "tasks" || input.mode === "milestones") {
    return input.status === "Done" ? 100 : 0;
  }

  return 0;
}

export function calculateGoalProgress(input: GoalProgressInput): number {
  if (input.mode === "manual") {
    return clampPercent(input.manualProgressValue ?? 0);
  }

  if (input.mode === "focus_time") {
    return ratioPercent(input.focusMinutesTotal, input.focusTargetMinutes);
  }

  if (input.mode === "milestones") {
    const done = input.milestones.filter((milestone) => milestone.status === "done").length;
    return ratioPercent(done, input.milestones.length);
  }

  if (input.mode === "tasks") {
    const done = input.linkedItems.filter((item) => item.status === "Done").length;
    return ratioPercent(done, input.linkedItems.length);
  }

  if (input.mode === "streak") {
    const done = input.linkedItems.filter((item) => item.status === "Done").length;
    return ratioPercent(done, input.linkedItems.length);
  }

  return 0;
}
```

- [ ] **Step 5: Run tests and verify pass**

Run:

```powershell
npm test -- src/domain/progress.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add personal-progress-workspace/src/domain
git commit -m "feat: add progress domain calculations"
```

---

### Task 3: Add Supabase Schema And RLS

**Files:**
- Create: `personal-progress-workspace/supabase/migrations/202605050001_initial_schema.sql`

- [ ] **Step 1: Create the migration**

Create `supabase/migrations/202605050001_initial_schema.sql`:

```sql
create extension if not exists pgcrypto;

create or replace function public.set_updated_at()
returns trigger
language plpgsql
as $$
begin
  new.updated_at = now();
  return new;
end;
$$;

create table public.workspaces (
  id uuid primary key default gen_random_uuid(),
  name text not null,
  owner_user_id uuid not null references auth.users(id) on delete cascade,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table public.workspace_members (
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  role text not null check (role in ('owner', 'member')),
  created_at timestamptz not null default now(),
  primary key (workspace_id, user_id)
);

create table public.boards (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  name text not null,
  description text,
  sort_order numeric not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table public.board_columns (
  id uuid primary key default gen_random_uuid(),
  board_id uuid not null references public.boards(id) on delete cascade,
  name text not null,
  status_key text not null check (status_key in ('Inbox', 'Planned', 'Doing', 'Review', 'Done')),
  sort_order numeric not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (board_id, status_key)
);

create table public.goals (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  title text not null,
  description text,
  status text not null default 'active' check (status in ('active', 'paused', 'completed')),
  target_date date,
  progress_mode text not null default 'tasks' check (progress_mode in ('tasks', 'milestones', 'focus_time', 'streak', 'manual')),
  manual_progress_value integer check (manual_progress_value between 0 and 100),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table public.items (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  board_id uuid not null references public.boards(id) on delete cascade,
  column_id uuid not null references public.board_columns(id) on delete restrict,
  goal_id uuid references public.goals(id) on delete set null,
  title text not null,
  description text,
  type text not null default 'task' check (type in ('task', 'content', 'learning', 'habit', 'personal')),
  tags text[] not null default '{}'::text[],
  priority text not null default 'medium' check (priority in ('low', 'medium', 'high', 'urgent')),
  status text not null default 'Inbox' check (status in ('Inbox', 'Planned', 'Doing', 'Review', 'Done')),
  scheduled_date date,
  due_date date,
  estimate_minutes integer check (estimate_minutes is null or estimate_minutes >= 0),
  focus_minutes_total integer not null default 0 check (focus_minutes_total >= 0),
  progress_mode text not null default 'tasks' check (progress_mode in ('tasks', 'milestones', 'focus_time', 'streak', 'manual')),
  progress_value integer check (progress_value between 0 and 100),
  sort_order numeric not null default 0,
  created_by uuid not null references auth.users(id) on delete cascade,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table public.milestones (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  goal_id uuid not null references public.goals(id) on delete cascade,
  title text not null,
  status text not null default 'active' check (status in ('active', 'done')),
  due_date date,
  sort_order numeric not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table public.focus_sessions (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  item_id uuid references public.items(id) on delete set null,
  goal_id uuid references public.goals(id) on delete set null,
  started_at timestamptz not null,
  ended_at timestamptz not null,
  duration_minutes integer not null check (duration_minutes >= 0),
  notes text,
  created_at timestamptz not null default now()
);

create table public.habit_logs (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  item_id uuid references public.items(id) on delete cascade,
  goal_id uuid references public.goals(id) on delete set null,
  log_date date not null,
  value integer not null default 1,
  created_at timestamptz not null default now(),
  unique (workspace_id, item_id, log_date)
);

create table public.activity_events (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  actor_user_id uuid not null references auth.users(id) on delete cascade,
  entity_type text not null,
  entity_id uuid not null,
  event_type text not null,
  metadata jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);

create index boards_workspace_id_idx on public.boards(workspace_id);
create index board_columns_board_id_idx on public.board_columns(board_id);
create index items_workspace_status_idx on public.items(workspace_id, status);
create index items_board_column_order_idx on public.items(board_id, column_id, sort_order);
create index items_tags_idx on public.items using gin(tags);
create index goals_workspace_id_idx on public.goals(workspace_id);
create index milestones_goal_id_idx on public.milestones(goal_id);
create index focus_sessions_workspace_started_idx on public.focus_sessions(workspace_id, started_at desc);
create index habit_logs_workspace_date_idx on public.habit_logs(workspace_id, log_date desc);
create index activity_events_workspace_created_idx on public.activity_events(workspace_id, created_at desc);

create trigger workspaces_updated_at before update on public.workspaces
  for each row execute function public.set_updated_at();
create trigger boards_updated_at before update on public.boards
  for each row execute function public.set_updated_at();
create trigger board_columns_updated_at before update on public.board_columns
  for each row execute function public.set_updated_at();
create trigger goals_updated_at before update on public.goals
  for each row execute function public.set_updated_at();
create trigger items_updated_at before update on public.items
  for each row execute function public.set_updated_at();
create trigger milestones_updated_at before update on public.milestones
  for each row execute function public.set_updated_at();

alter table public.workspaces enable row level security;
alter table public.workspace_members enable row level security;
alter table public.boards enable row level security;
alter table public.board_columns enable row level security;
alter table public.goals enable row level security;
alter table public.items enable row level security;
alter table public.milestones enable row level security;
alter table public.focus_sessions enable row level security;
alter table public.habit_logs enable row level security;
alter table public.activity_events enable row level security;

create or replace function public.is_workspace_member(target_workspace_id uuid)
returns boolean
language sql
security definer
set search_path = public
as $$
  select exists (
    select 1
    from public.workspace_members wm
    where wm.workspace_id = target_workspace_id
      and wm.user_id = auth.uid()
  );
$$;

create policy "workspace owners can create workspace"
on public.workspaces for insert
with check (owner_user_id = auth.uid());

create policy "workspace members can read workspace"
on public.workspaces for select
using (public.is_workspace_member(id));

create policy "workspace owners can update workspace"
on public.workspaces for update
using (owner_user_id = auth.uid())
with check (owner_user_id = auth.uid());

create policy "workspace owners can create owner membership"
on public.workspace_members for insert
with check (
  user_id = auth.uid()
  and role = 'owner'
  and exists (
    select 1 from public.workspaces w
    where w.id = workspace_id
      and w.owner_user_id = auth.uid()
  )
);

create policy "workspace members can read memberships"
on public.workspace_members for select
using (public.is_workspace_member(workspace_id));

create policy "members manage boards"
on public.boards for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage goals"
on public.goals for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage items"
on public.items for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage milestones"
on public.milestones for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage focus sessions"
on public.focus_sessions for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage habit logs"
on public.habit_logs for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage activity events"
on public.activity_events for all
using (public.is_workspace_member(workspace_id))
with check (public.is_workspace_member(workspace_id));

create policy "members manage board columns through board"
on public.board_columns for all
using (
  exists (
    select 1 from public.boards b
    where b.id = board_id
      and public.is_workspace_member(b.workspace_id)
  )
)
with check (
  exists (
    select 1 from public.boards b
    where b.id = board_id
      and public.is_workspace_member(b.workspace_id)
  )
);
```

- [ ] **Step 2: Validate migration syntax locally when Supabase CLI is available**

Run:

```powershell
supabase db lint --workdir personal-progress-workspace
```

Expected: no syntax or policy errors. If Supabase CLI is not installed, run the SQL in the hosted Supabase SQL editor before app integration and record the successful execution in the task notes.

- [ ] **Step 3: Commit**

```powershell
git add personal-progress-workspace/supabase/migrations/202605050001_initial_schema.sql
git commit -m "feat: add Supabase workspace schema"
```

---

### Task 4: Add Supabase Client, Auth, And Workspace Bootstrap

**Files:**
- Create: `personal-progress-workspace/src/lib/env.ts`
- Create: `personal-progress-workspace/src/lib/supabase.ts`
- Create: `personal-progress-workspace/src/lib/queryClient.ts`
- Create: `personal-progress-workspace/src/features/auth/AuthProvider.tsx`
- Create: `personal-progress-workspace/src/features/auth/SignInPage.tsx`
- Create: `personal-progress-workspace/src/features/workspaces/workspaceApi.ts`
- Create: `personal-progress-workspace/src/features/workspaces/useWorkspace.ts`
- Modify: `personal-progress-workspace/src/App.tsx`

- [ ] **Step 1: Create environment helper**

`src/lib/env.ts`:

```ts
const supabaseUrl = import.meta.env.VITE_SUPABASE_URL as string | undefined;
const supabaseAnonKey = import.meta.env.VITE_SUPABASE_ANON_KEY as string | undefined;

export const env = {
  supabaseUrl: supabaseUrl ?? "",
  supabaseAnonKey: supabaseAnonKey ?? "",
  hasSupabaseConfig: Boolean(supabaseUrl && supabaseAnonKey),
};
```

- [ ] **Step 2: Create Supabase client**

`src/lib/supabase.ts`:

```ts
import { createClient } from "@supabase/supabase-js";
import { env } from "./env";

export const supabase = createClient(env.supabaseUrl, env.supabaseAnonKey, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: true,
  },
});
```

- [ ] **Step 3: Create query client**

`src/lib/queryClient.ts`:

```ts
import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 20_000,
      retry: 1,
    },
  },
});
```

- [ ] **Step 4: Create auth provider**

`src/features/auth/AuthProvider.tsx`:

```tsx
import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { Session, User } from "@supabase/supabase-js";
import { supabase } from "../../lib/supabase";

interface AuthState {
  session: Session | null;
  user: User | null;
  loading: boolean;
}

const AuthContext = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = useState<Session | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let mounted = true;

    supabase.auth.getSession().then(({ data }) => {
      if (!mounted) return;
      setSession(data.session);
      setLoading(false);
    });

    const { data: listener } = supabase.auth.onAuthStateChange((_event, nextSession) => {
      setSession(nextSession);
      setLoading(false);
    });

    return () => {
      mounted = false;
      listener.subscription.unsubscribe();
    };
  }, []);

  const value = useMemo<AuthState>(
    () => ({ session, user: session?.user ?? null, loading }),
    [loading, session],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const value = useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used within AuthProvider");
  return value;
}
```

- [ ] **Step 5: Create sign-in page**

`src/features/auth/SignInPage.tsx`:

```tsx
import { FormEvent, useState } from "react";
import { LogIn } from "lucide-react";
import { supabase } from "../../lib/supabase";

export function SignInPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);

    const { error: signInError } = await supabase.auth.signInWithPassword({ email, password });

    if (signInError) setError(signInError.message);
    setSubmitting(false);
  }

  return (
    <main className="auth-page">
      <form className="auth-card" onSubmit={handleSubmit}>
        <p className="eyebrow">Personal Progress Workspace</p>
        <h1>Sign in to your command center</h1>
        <label>
          Email
          <input value={email} onChange={(event) => setEmail(event.target.value)} type="email" required />
        </label>
        <label>
          Password
          <input value={password} onChange={(event) => setPassword(event.target.value)} type="password" required />
        </label>
        {error ? <p className="form-error">{error}</p> : null}
        <button type="submit" disabled={submitting}>
          <LogIn size={18} />
          {submitting ? "Signing in" : "Sign in"}
        </button>
      </form>
    </main>
  );
}
```

- [ ] **Step 6: Create workspace bootstrap API**

`src/features/workspaces/workspaceApi.ts`:

```ts
import type { User } from "@supabase/supabase-js";
import { supabase } from "../../lib/supabase";

export interface WorkspaceRecord {
  id: string;
  name: string;
  owner_user_id: string;
}

const DEFAULT_COLUMNS = ["Inbox", "Planned", "Doing", "Review", "Done"] as const;

export async function getOrCreatePersonalWorkspace(user: User): Promise<WorkspaceRecord> {
  const { data: existing, error: existingError } = await supabase
    .from("workspace_members")
    .select("workspaces(id, name, owner_user_id)")
    .eq("user_id", user.id)
    .limit(1)
    .maybeSingle();

  if (existingError) throw existingError;

  const workspace = existing?.workspaces as WorkspaceRecord | null;
  if (workspace) return workspace;

  const { data: createdWorkspace, error: workspaceError } = await supabase
    .from("workspaces")
    .insert({ name: "Personal Workspace", owner_user_id: user.id })
    .select("id, name, owner_user_id")
    .single();

  if (workspaceError) throw workspaceError;

  const { error: memberError } = await supabase
    .from("workspace_members")
    .insert({ workspace_id: createdWorkspace.id, user_id: user.id, role: "owner" });

  if (memberError) throw memberError;

  const { data: board, error: boardError } = await supabase
    .from("boards")
    .insert({ workspace_id: createdWorkspace.id, name: "Main Board", sort_order: 0 })
    .select("id")
    .single();

  if (boardError) throw boardError;

  const { error: columnsError } = await supabase.from("board_columns").insert(
    DEFAULT_COLUMNS.map((column, index) => ({
      board_id: board.id,
      name: column,
      status_key: column,
      sort_order: index,
    })),
  );

  if (columnsError) throw columnsError;

  return createdWorkspace;
}
```

- [ ] **Step 7: Create workspace hook**

`src/features/workspaces/useWorkspace.ts`:

```ts
import { useQuery } from "@tanstack/react-query";
import { useAuth } from "../auth/AuthProvider";
import { getOrCreatePersonalWorkspace } from "./workspaceApi";

export function useWorkspace() {
  const { user } = useAuth();

  return useQuery({
    queryKey: ["workspace", user?.id],
    queryFn: () => {
      if (!user) throw new Error("User is required to load workspace");
      return getOrCreatePersonalWorkspace(user);
    },
    enabled: Boolean(user),
  });
}
```

- [ ] **Step 8: Wire auth in `src/App.tsx`**

```tsx
import { AuthProvider, useAuth } from "./features/auth/AuthProvider";
import { SignInPage } from "./features/auth/SignInPage";
import { useWorkspace } from "./features/workspaces/useWorkspace";

function AppContent() {
  const { user, loading: authLoading } = useAuth();
  const workspace = useWorkspace();

  if (authLoading) return <main className="app-shell">Loading session...</main>;
  if (!user) return <SignInPage />;
  if (workspace.isLoading) return <main className="app-shell">Preparing workspace...</main>;
  if (workspace.isError) return <main className="app-shell">Workspace failed to load.</main>;

  return (
    <main className="app-shell">
      <section className="hero-panel">
        <p className="eyebrow">{workspace.data.name}</p>
        <h1>Workspace loaded</h1>
        <p>Board data connects in the next task.</p>
      </section>
    </main>
  );
}

export function App() {
  return (
    <AuthProvider>
      <AppContent />
    </AuthProvider>
  );
}
```

- [ ] **Step 9: Verify**

Run:

```powershell
npm run build
```

Expected: TypeScript and Vite build pass.

- [ ] **Step 10: Commit**

```powershell
git add personal-progress-workspace/src personal-progress-workspace/.env.example personal-progress-workspace/package.json personal-progress-workspace/package-lock.json
git commit -m "feat: add Supabase auth bootstrap"
```

---

### Task 5: Add Board Data Utilities

**Files:**
- Create: `personal-progress-workspace/src/features/boards/boardUtils.ts`
- Create: `personal-progress-workspace/src/features/boards/boardUtils.test.ts`
- Create: `personal-progress-workspace/src/features/boards/boardApi.ts`

- [ ] **Step 1: Write failing board utility tests**

`src/features/boards/boardUtils.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { groupItemsByColumn, reorderAfterMove } from "./boardUtils";
import type { BoardColumn, WorkspaceItem } from "../../domain/types";

const columns: BoardColumn[] = [
  { id: "inbox", boardId: "board", name: "Inbox", statusKey: "Inbox", sortOrder: 0 },
  { id: "doing", boardId: "board", name: "Doing", statusKey: "Doing", sortOrder: 1 },
];

function item(id: string, columnId: string, sortOrder: number): WorkspaceItem {
  return {
    id,
    workspaceId: "workspace",
    boardId: "board",
    columnId,
    goalId: null,
    title: id,
    description: null,
    type: "task",
    tags: [],
    priority: "medium",
    status: columnId === "doing" ? "Doing" : "Inbox",
    scheduledDate: null,
    dueDate: null,
    estimateMinutes: null,
    focusMinutesTotal: 0,
    progressMode: "tasks",
    progressValue: null,
    sortOrder,
  };
}

describe("groupItemsByColumn", () => {
  it("returns every column with sorted items", () => {
    const result = groupItemsByColumn(columns, [item("b", "inbox", 2), item("a", "inbox", 1)]);
    expect(result[0].items.map((entry) => entry.id)).toEqual(["a", "b"]);
    expect(result[1].items).toEqual([]);
  });
});

describe("reorderAfterMove", () => {
  it("moves an item to a new column and assigns status and sort order", () => {
    const result = reorderAfterMove({
      items: [item("a", "inbox", 0), item("b", "doing", 0)],
      activeItemId: "a",
      targetColumn: columns[1],
      targetIndex: 1,
    });

    const moved = result.find((entry) => entry.id === "a");
    expect(moved?.columnId).toBe("doing");
    expect(moved?.status).toBe("Doing");
    expect(moved?.sortOrder).toBe(1);
  });
});
```

- [ ] **Step 2: Run tests and verify failure**

```powershell
npm test -- src/features/boards/boardUtils.test.ts
```

Expected: FAIL because `boardUtils.ts` does not exist.

- [ ] **Step 3: Create `boardUtils.ts`**

```ts
import type { BoardColumn, WorkspaceItem } from "../../domain/types";

export interface ColumnWithItems extends BoardColumn {
  items: WorkspaceItem[];
}

export function groupItemsByColumn(columns: BoardColumn[], items: WorkspaceItem[]): ColumnWithItems[] {
  return [...columns]
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .map((column) => ({
      ...column,
      items: items
        .filter((item) => item.columnId === column.id)
        .sort((a, b) => a.sortOrder - b.sortOrder),
    }));
}

export function reorderAfterMove(input: {
  items: WorkspaceItem[];
  activeItemId: string;
  targetColumn: BoardColumn;
  targetIndex: number;
}): WorkspaceItem[] {
  const active = input.items.find((item) => item.id === input.activeItemId);
  if (!active) return input.items;

  const withoutActive = input.items.filter((item) => item.id !== input.activeItemId);
  const targetColumnItems = withoutActive
    .filter((item) => item.columnId === input.targetColumn.id)
    .sort((a, b) => a.sortOrder - b.sortOrder);

  const moved: WorkspaceItem = {
    ...active,
    columnId: input.targetColumn.id,
    status: input.targetColumn.statusKey,
  };

  const insertAt = Math.max(0, Math.min(input.targetIndex, targetColumnItems.length));
  targetColumnItems.splice(insertAt, 0, moved);

  const targetIds = new Set(targetColumnItems.map((item) => item.id));

  return [
    ...withoutActive.filter((item) => !targetIds.has(item.id)),
    ...targetColumnItems.map((item, index) => ({ ...item, sortOrder: index })),
  ];
}
```

- [ ] **Step 4: Create `boardApi.ts`**

```ts
import { supabase } from "../../lib/supabase";
import type { BoardColumn, WorkspaceItem } from "../../domain/types";

export interface BoardData {
  boardId: string;
  columns: BoardColumn[];
  items: WorkspaceItem[];
}

function mapColumn(row: any): BoardColumn {
  return {
    id: row.id,
    boardId: row.board_id,
    name: row.name,
    statusKey: row.status_key,
    sortOrder: Number(row.sort_order),
  };
}

function mapItem(row: any): WorkspaceItem {
  return {
    id: row.id,
    workspaceId: row.workspace_id,
    boardId: row.board_id,
    columnId: row.column_id,
    goalId: row.goal_id,
    title: row.title,
    description: row.description,
    type: row.type,
    tags: row.tags ?? [],
    priority: row.priority,
    status: row.status,
    scheduledDate: row.scheduled_date,
    dueDate: row.due_date,
    estimateMinutes: row.estimate_minutes,
    focusMinutesTotal: row.focus_minutes_total,
    progressMode: row.progress_mode,
    progressValue: row.progress_value,
    sortOrder: Number(row.sort_order),
  };
}

export async function loadPrimaryBoard(workspaceId: string): Promise<BoardData> {
  const { data: boards, error: boardError } = await supabase
    .from("boards")
    .select("id")
    .eq("workspace_id", workspaceId)
    .order("sort_order", { ascending: true })
    .limit(1);

  if (boardError) throw boardError;
  const boardId = boards?.[0]?.id;
  if (!boardId) throw new Error("No board exists for workspace");

  const [{ data: columns, error: columnsError }, { data: items, error: itemsError }] = await Promise.all([
    supabase.from("board_columns").select("*").eq("board_id", boardId).order("sort_order", { ascending: true }),
    supabase.from("items").select("*").eq("board_id", boardId).order("sort_order", { ascending: true }),
  ]);

  if (columnsError) throw columnsError;
  if (itemsError) throw itemsError;

  return {
    boardId,
    columns: (columns ?? []).map(mapColumn),
    items: (items ?? []).map(mapItem),
  };
}

export async function updateItemPosition(item: WorkspaceItem): Promise<void> {
  const { error } = await supabase
    .from("items")
    .update({
      column_id: item.columnId,
      status: item.status,
      sort_order: item.sortOrder,
    })
    .eq("id", item.id);

  if (error) throw error;
}
```

- [ ] **Step 5: Run tests and build**

```powershell
npm test -- src/features/boards/boardUtils.test.ts
npm run build
```

Expected: tests and build pass.

- [ ] **Step 6: Commit**

```powershell
git add personal-progress-workspace/src/features/boards personal-progress-workspace/src/domain
git commit -m "feat: add board data utilities"
```

---

### Task 6: Build The Board UI And Drag-Drop Flow

**Files:**
- Create: `personal-progress-workspace/src/features/boards/BoardPage.tsx`
- Create: `personal-progress-workspace/src/features/boards/BoardColumn.tsx`
- Create: `personal-progress-workspace/src/features/boards/ItemCard.tsx`
- Modify: `personal-progress-workspace/src/App.tsx`
- Modify: `personal-progress-workspace/src/styles/app.css`

- [ ] **Step 1: Create item card**

`src/features/boards/ItemCard.tsx`:

```tsx
import { CalendarDays, Clock3, Flag } from "lucide-react";
import type { WorkspaceItem } from "../../domain/types";
import { calculateItemProgress } from "../../domain/progress";

export function ItemCard({ item, onOpen }: { item: WorkspaceItem; onOpen: (item: WorkspaceItem) => void }) {
  const progress = calculateItemProgress({
    mode: item.progressMode,
    status: item.status,
    progressValue: item.progressValue,
    estimateMinutes: item.estimateMinutes,
    focusMinutesTotal: item.focusMinutesTotal,
  });

  return (
    <button className="item-card" type="button" onClick={() => onOpen(item)}>
      <div className="item-card__topline">
        <span className={`type-pill type-pill--${item.type}`}>{item.type}</span>
        <span className={`priority-dot priority-dot--${item.priority}`} aria-label={`${item.priority} priority`} />
      </div>
      <h3>{item.title}</h3>
      <div className="item-card__meta">
        {item.dueDate ? <span><CalendarDays size={14} /> {item.dueDate}</span> : null}
        {item.estimateMinutes ? <span><Clock3 size={14} /> {item.estimateMinutes}m</span> : null}
        {item.goalId ? <span><Flag size={14} /> Goal</span> : null}
      </div>
      <div className="progress-bar" aria-label={`${progress}% progress`}>
        <span style={{ width: `${progress}%` }} />
      </div>
    </button>
  );
}
```

- [ ] **Step 2: Create board column**

`src/features/boards/BoardColumn.tsx`:

```tsx
import { useDroppable } from "@dnd-kit/core";
import type { ColumnWithItems } from "./boardUtils";
import type { WorkspaceItem } from "../../domain/types";
import { ItemCard } from "./ItemCard";

export function BoardColumn({
  column,
  onOpenItem,
}: {
  column: ColumnWithItems;
  onOpenItem: (item: WorkspaceItem) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: column.id });

  return (
    <section ref={setNodeRef} className={`board-column ${isOver ? "board-column--over" : ""}`}>
      <header>
        <h2>{column.name}</h2>
        <span>{column.items.length}</span>
      </header>
      <div className="board-column__cards">
        {column.items.map((item) => (
          <ItemCard key={item.id} item={item} onOpen={onOpenItem} />
        ))}
      </div>
    </section>
  );
}
```

- [ ] **Step 3: Create board page**

`src/features/boards/BoardPage.tsx`:

```tsx
import { DndContext, DragEndEvent } from "@dnd-kit/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { WorkspaceItem } from "../../domain/types";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { BoardColumn } from "./BoardColumn";
import { loadPrimaryBoard, updateItemPosition } from "./boardApi";
import { groupItemsByColumn, reorderAfterMove } from "./boardUtils";

export function BoardPage({ workspace }: { workspace: WorkspaceRecord }) {
  const queryClient = useQueryClient();
  const [openItem, setOpenItem] = useState<WorkspaceItem | null>(null);

  const boardQuery = useQuery({
    queryKey: ["board", workspace.id],
    queryFn: () => loadPrimaryBoard(workspace.id),
  });

  const moveMutation = useMutation({
    mutationFn: updateItemPosition,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["board", workspace.id] }),
  });

  const columns = useMemo(() => {
    if (!boardQuery.data) return [];
    return groupItemsByColumn(boardQuery.data.columns, boardQuery.data.items);
  }, [boardQuery.data]);

  function handleDragEnd(event: DragEndEvent) {
    if (!boardQuery.data || !event.over) return;
    const activeItemId = String(event.active.id);
    const targetColumn = boardQuery.data.columns.find((column) => column.id === String(event.over?.id));
    if (!targetColumn) return;

    const currentTargetItems = boardQuery.data.items.filter((item) => item.columnId === targetColumn.id);
    const nextItems = reorderAfterMove({
      items: boardQuery.data.items,
      activeItemId,
      targetColumn,
      targetIndex: currentTargetItems.length,
    });

    const moved = nextItems.find((item) => item.id === activeItemId);
    if (!moved) return;

    queryClient.setQueryData(["board", workspace.id], { ...boardQuery.data, items: nextItems });
    moveMutation.mutate(moved);
  }

  if (boardQuery.isLoading) return <section className="board-state">Loading board...</section>;
  if (boardQuery.isError) return <section className="board-state">Board failed to load.</section>;

  return (
    <section className="workspace-layout">
      <aside className="workspace-sidebar">
        <p className="eyebrow">{workspace.name}</p>
        <h1>Main Board</h1>
      </aside>
      <DndContext onDragEnd={handleDragEnd}>
        <div className="board-grid">
          {columns.map((column) => (
            <BoardColumn key={column.id} column={column} onOpenItem={setOpenItem} />
          ))}
        </div>
      </DndContext>
      {openItem ? (
        <aside className="detail-drawer">
          <button type="button" onClick={() => setOpenItem(null)}>Close</button>
          <h2>{openItem.title}</h2>
          <p>{openItem.description ?? "No description yet."}</p>
        </aside>
      ) : null}
    </section>
  );
}
```

- [ ] **Step 4: Wire board page into `App.tsx`**

Replace the loaded state with:

```tsx
return <BoardPage workspace={workspace.data} />;
```

Add import:

```tsx
import { BoardPage } from "./features/boards/BoardPage";
```

- [ ] **Step 5: Add board styles**

Append to `src/styles/app.css`:

```css
.workspace-layout {
  min-height: 100vh;
  display: grid;
  grid-template-columns: 260px minmax(0, 1fr);
  gap: 20px;
}

.workspace-sidebar,
.board-column,
.detail-drawer,
.auth-card {
  border: 1px solid var(--line);
  border-radius: 8px;
  background: rgba(14, 22, 38, 0.92);
  box-shadow: var(--shadow);
}

.workspace-sidebar {
  padding: 20px;
}

.board-grid {
  display: grid;
  grid-template-columns: repeat(5, minmax(220px, 1fr));
  gap: 14px;
  overflow-x: auto;
  padding-bottom: 20px;
}

.board-column {
  min-height: 70vh;
  padding: 14px;
}

.board-column--over {
  border-color: var(--blue);
}

.board-column header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 14px;
}

.board-column h2 {
  margin: 0;
  font-size: 16px;
}

.board-column__cards {
  display: grid;
  gap: 10px;
}

.item-card {
  width: 100%;
  padding: 14px;
  border: 1px solid #2b3954;
  border-radius: 8px;
  background: #111a2c;
  color: var(--text);
  text-align: left;
  cursor: pointer;
}

.item-card:hover {
  border-color: var(--blue);
}

.item-card h3 {
  margin: 10px 0;
  font-size: 15px;
}

.item-card__topline,
.item-card__meta {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.item-card__meta {
  color: var(--muted);
  font-size: 12px;
}

.item-card__meta span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.type-pill {
  padding: 3px 7px;
  border-radius: 999px;
  background: rgba(56, 189, 248, 0.14);
  color: var(--blue);
  font-size: 11px;
  font-weight: 700;
}

.priority-dot {
  width: 9px;
  height: 9px;
  border-radius: 999px;
  background: var(--muted);
}

.priority-dot--high,
.priority-dot--urgent {
  background: var(--rose);
}

.priority-dot--medium {
  background: var(--amber);
}

.priority-dot--low {
  background: var(--green);
}

.progress-bar {
  height: 6px;
  margin-top: 12px;
  overflow: hidden;
  border-radius: 999px;
  background: #26344d;
}

.progress-bar span {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--blue), var(--green));
}

.detail-drawer {
  position: fixed;
  top: 24px;
  right: 24px;
  width: min(420px, calc(100vw - 48px));
  padding: 20px;
}

.board-state {
  padding: 32px;
}
```

- [ ] **Step 6: Verify**

Run:

```powershell
npm run build
```

Expected: TypeScript passes and Vite build succeeds.

- [ ] **Step 7: Commit**

```powershell
git add personal-progress-workspace/src
git commit -m "feat: add board workspace UI"
```

---

### Task 7: Add Item Create/Edit Drawer

**Files:**
- Create: `personal-progress-workspace/src/features/boards/ItemForm.tsx`
- Create: `personal-progress-workspace/src/features/boards/ItemDrawer.tsx`
- Modify: `personal-progress-workspace/src/features/boards/boardApi.ts`
- Modify: `personal-progress-workspace/src/features/boards/BoardPage.tsx`

- [ ] **Step 1: Extend board API with create and update**

Add to `boardApi.ts`:

```ts
export interface ItemInput {
  workspaceId: string;
  boardId: string;
  columnId: string;
  title: string;
  description: string | null;
  type: string;
  tags: string[];
  priority: string;
  status: string;
  scheduledDate: string | null;
  dueDate: string | null;
  estimateMinutes: number | null;
  progressMode: string;
  progressValue: number | null;
  createdBy: string;
}

export async function createItem(input: ItemInput): Promise<WorkspaceItem> {
  const { data, error } = await supabase
    .from("items")
    .insert({
      workspace_id: input.workspaceId,
      board_id: input.boardId,
      column_id: input.columnId,
      title: input.title,
      description: input.description,
      type: input.type,
      tags: input.tags,
      priority: input.priority,
      status: input.status,
      scheduled_date: input.scheduledDate,
      due_date: input.dueDate,
      estimate_minutes: input.estimateMinutes,
      progress_mode: input.progressMode,
      progress_value: input.progressValue,
      created_by: input.createdBy,
    })
    .select("*")
    .single();

  if (error) throw error;
  return mapItem(data);
}

export async function updateItem(itemId: string, input: Partial<ItemInput>): Promise<WorkspaceItem> {
  const { data, error } = await supabase
    .from("items")
    .update({
      title: input.title,
      description: input.description,
      type: input.type,
      tags: input.tags,
      priority: input.priority,
      scheduled_date: input.scheduledDate,
      due_date: input.dueDate,
      estimate_minutes: input.estimateMinutes,
      progress_mode: input.progressMode,
      progress_value: input.progressValue,
    })
    .eq("id", itemId)
    .select("*")
    .single();

  if (error) throw error;
  return mapItem(data);
}
```

- [ ] **Step 2: Create `ItemForm.tsx`**

```tsx
import { FormEvent, useState } from "react";
import type { WorkspaceItem } from "../../domain/types";
import type { ItemInput } from "./boardApi";

export function ItemForm({
  initialItem,
  defaults,
  onSubmit,
}: {
  initialItem?: WorkspaceItem | null;
  defaults: Pick<ItemInput, "workspaceId" | "boardId" | "columnId" | "status" | "createdBy">;
  onSubmit: (input: ItemInput) => void;
}) {
  const [title, setTitle] = useState(initialItem?.title ?? "");
  const [description, setDescription] = useState(initialItem?.description ?? "");
  const [type, setType] = useState(initialItem?.type ?? "task");
  const [priority, setPriority] = useState(initialItem?.priority ?? "medium");
  const [tags, setTags] = useState((initialItem?.tags ?? []).join(", "));
  const [dueDate, setDueDate] = useState(initialItem?.dueDate ?? "");
  const [scheduledDate, setScheduledDate] = useState(initialItem?.scheduledDate ?? "");
  const [estimateMinutes, setEstimateMinutes] = useState(initialItem?.estimateMinutes?.toString() ?? "");
  const [progressMode, setProgressMode] = useState(initialItem?.progressMode ?? "tasks");

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSubmit({
      ...defaults,
      title,
      description: description.trim() ? description : null,
      type,
      tags: tags.split(",").map((tag) => tag.trim()).filter(Boolean),
      priority,
      scheduledDate: scheduledDate || null,
      dueDate: dueDate || null,
      estimateMinutes: estimateMinutes ? Number(estimateMinutes) : null,
      progressMode,
      progressValue: initialItem?.progressValue ?? null,
    });
  }

  return (
    <form className="item-form" onSubmit={handleSubmit}>
      <label>
        Title
        <input value={title} onChange={(event) => setTitle(event.target.value)} required />
      </label>
      <label>
        Description
        <textarea value={description} onChange={(event) => setDescription(event.target.value)} rows={4} />
      </label>
      <div className="form-grid">
        <label>
          Type
          <select value={type} onChange={(event) => setType(event.target.value)}>
            <option value="task">Task</option>
            <option value="content">Content</option>
            <option value="learning">Learning</option>
            <option value="habit">Habit</option>
            <option value="personal">Personal</option>
          </select>
        </label>
        <label>
          Priority
          <select value={priority} onChange={(event) => setPriority(event.target.value)}>
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
            <option value="urgent">Urgent</option>
          </select>
        </label>
      </div>
      <label>
        Tags
        <input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="content, learning" />
      </label>
      <div className="form-grid">
        <label>
          Scheduled
          <input type="date" value={scheduledDate} onChange={(event) => setScheduledDate(event.target.value)} />
        </label>
        <label>
          Due
          <input type="date" value={dueDate} onChange={(event) => setDueDate(event.target.value)} />
        </label>
      </div>
      <div className="form-grid">
        <label>
          Estimate minutes
          <input type="number" min="0" value={estimateMinutes} onChange={(event) => setEstimateMinutes(event.target.value)} />
        </label>
        <label>
          Progress mode
          <select value={progressMode} onChange={(event) => setProgressMode(event.target.value)}>
            <option value="tasks">Tasks</option>
            <option value="milestones">Milestones</option>
            <option value="focus_time">Focus time</option>
            <option value="streak">Streak</option>
            <option value="manual">Manual</option>
          </select>
        </label>
      </div>
      <button type="submit">{initialItem ? "Save item" : "Create item"}</button>
    </form>
  );
}
```

- [ ] **Step 3: Create `ItemDrawer.tsx`**

```tsx
import type { User } from "@supabase/supabase-js";
import type { WorkspaceItem } from "../../domain/types";
import type { BoardData, ItemInput } from "./boardApi";
import { ItemForm } from "./ItemForm";

export function ItemDrawer({
  user,
  workspaceId,
  board,
  item,
  onClose,
  onSubmit,
}: {
  user: User;
  workspaceId: string;
  board: BoardData;
  item: WorkspaceItem | null;
  onClose: () => void;
  onSubmit: (itemId: string | null, input: ItemInput) => void;
}) {
  const defaultColumn = board.columns[0];

  return (
    <aside className="detail-drawer">
      <button type="button" onClick={onClose}>Close</button>
      <h2>{item ? "Edit item" : "Create item"}</h2>
      <ItemForm
        initialItem={item}
        defaults={{
          workspaceId,
          boardId: board.boardId,
          columnId: item?.columnId ?? defaultColumn.id,
          status: item?.status ?? defaultColumn.statusKey,
          createdBy: user.id,
        }}
        onSubmit={(input) => onSubmit(item?.id ?? null, input)}
      />
    </aside>
  );
}
```

- [ ] **Step 4: Wire mutations in `BoardPage.tsx`**

Add create and update mutations:

```tsx
const saveMutation = useMutation({
  mutationFn: ({ itemId, input }: { itemId: string | null; input: ItemInput }) =>
    itemId ? updateItem(itemId, input) : createItem(input),
  onSuccess: () => {
    setOpenItem(null);
    queryClient.invalidateQueries({ queryKey: ["board", workspace.id] });
  },
});
```

Add imports:

```tsx
import { createItem, updateItem, type ItemInput } from "./boardApi";
import { ItemDrawer } from "./ItemDrawer";
import { useAuth } from "../auth/AuthProvider";
```

Use `const { user } = useAuth();` and replace the drawer block with:

```tsx
{openItem !== undefined && boardQuery.data && user ? (
  <ItemDrawer
    user={user}
    workspaceId={workspace.id}
    board={boardQuery.data}
    item={openItem}
    onClose={() => setOpenItem(null)}
    onSubmit={(itemId, input) => saveMutation.mutate({ itemId, input })}
  />
) : null}
```

- [ ] **Step 5: Verify**

Run:

```powershell
npm run build
```

Expected: build passes.

- [ ] **Step 6: Commit**

```powershell
git add personal-progress-workspace/src/features/boards
git commit -m "feat: add item drawer editing"
```

---

### Task 8: Add Today Panel And Focus Logging

**Files:**
- Create: `personal-progress-workspace/src/domain/dates.ts`
- Create: `personal-progress-workspace/src/features/today/todayUtils.ts`
- Create: `personal-progress-workspace/src/features/today/todayUtils.test.ts`
- Create: `personal-progress-workspace/src/features/today/FocusLogger.tsx`
- Create: `personal-progress-workspace/src/features/today/TodayPanel.tsx`
- Modify: `personal-progress-workspace/src/features/boards/boardApi.ts`
- Modify: `personal-progress-workspace/src/features/boards/BoardPage.tsx`

- [ ] **Step 1: Write failing Today tests**

`src/features/today/todayUtils.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { WorkspaceItem } from "../../domain/types";
import { splitTodayItems } from "./todayUtils";

function item(id: string, scheduledDate: string | null, dueDate: string | null): WorkspaceItem {
  return {
    id,
    workspaceId: "workspace",
    boardId: "board",
    columnId: "column",
    goalId: null,
    title: id,
    description: null,
    type: "task",
    tags: [],
    priority: "medium",
    status: "Planned",
    scheduledDate,
    dueDate,
    estimateMinutes: null,
    focusMinutesTotal: 0,
    progressMode: "tasks",
    progressValue: null,
    sortOrder: 0,
  };
}

describe("splitTodayItems", () => {
  it("separates scheduled today and overdue unfinished items", () => {
    const result = splitTodayItems(
      [item("today", "2026-05-05", null), item("late", null, "2026-05-04"), item("future", null, "2026-05-06")],
      "2026-05-05",
    );

    expect(result.today.map((entry) => entry.id)).toEqual(["today"]);
    expect(result.overdue.map((entry) => entry.id)).toEqual(["late"]);
  });
});
```

- [ ] **Step 2: Create date helper**

`src/domain/dates.ts`:

```ts
import { format } from "date-fns";

export function todayIsoDate(now = new Date()): string {
  return format(now, "yyyy-MM-dd");
}
```

- [ ] **Step 3: Create Today utilities**

`src/features/today/todayUtils.ts`:

```ts
import type { WorkspaceItem } from "../../domain/types";

export function splitTodayItems(items: WorkspaceItem[], today: string) {
  return {
    today: items.filter((item) => item.status !== "Done" && item.scheduledDate === today),
    overdue: items.filter((item) => item.status !== "Done" && item.dueDate !== null && item.dueDate < today),
  };
}
```

- [ ] **Step 4: Extend board API with focus logging**

Add:

```ts
export async function logFocusSession(input: {
  workspaceId: string;
  itemId: string;
  goalId: string | null;
  startedAt: string;
  endedAt: string;
  durationMinutes: number;
}): Promise<void> {
  const { error: sessionError } = await supabase.from("focus_sessions").insert({
    workspace_id: input.workspaceId,
    item_id: input.itemId,
    goal_id: input.goalId,
    started_at: input.startedAt,
    ended_at: input.endedAt,
    duration_minutes: input.durationMinutes,
  });

  if (sessionError) throw sessionError;

  const { error: itemError } = await supabase.rpc("increment_item_focus_minutes", {
    target_item_id: input.itemId,
    minutes_to_add: input.durationMinutes,
  });

  if (itemError) {
    const { data: item, error: loadError } = await supabase
      .from("items")
      .select("focus_minutes_total")
      .eq("id", input.itemId)
      .single();

    if (loadError) throw loadError;

    const { error: updateError } = await supabase
      .from("items")
      .update({ focus_minutes_total: (item.focus_minutes_total ?? 0) + input.durationMinutes })
      .eq("id", input.itemId);

    if (updateError) throw updateError;
  }
}
```

Also add this SQL function to the migration before policies if the migration has not been applied:

```sql
create or replace function public.increment_item_focus_minutes(target_item_id uuid, minutes_to_add integer)
returns void
language sql
security definer
set search_path = public
as $$
  update public.items
  set focus_minutes_total = focus_minutes_total + greatest(minutes_to_add, 0)
  where id = target_item_id
    and public.is_workspace_member(workspace_id);
$$;
```

- [ ] **Step 5: Create focus logger**

`src/features/today/FocusLogger.tsx`:

```tsx
import { FormEvent, useState } from "react";
import type { WorkspaceItem } from "../../domain/types";

export function FocusLogger({
  items,
  onLog,
}: {
  items: WorkspaceItem[];
  onLog: (item: WorkspaceItem, minutes: number) => void;
}) {
  const [itemId, setItemId] = useState(items[0]?.id ?? "");
  const [minutes, setMinutes] = useState("25");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const item = items.find((entry) => entry.id === itemId);
    if (!item) return;
    onLog(item, Number(minutes));
  }

  return (
    <form className="focus-logger" onSubmit={submit}>
      <label>
        Focus item
        <select value={itemId} onChange={(event) => setItemId(event.target.value)}>
          {items.map((item) => <option key={item.id} value={item.id}>{item.title}</option>)}
        </select>
      </label>
      <label>
        Minutes
        <input type="number" min="1" value={minutes} onChange={(event) => setMinutes(event.target.value)} />
      </label>
      <button type="submit">Log focus</button>
    </form>
  );
}
```

- [ ] **Step 6: Create Today panel**

`src/features/today/TodayPanel.tsx`:

```tsx
import type { WorkspaceItem } from "../../domain/types";
import { todayIsoDate } from "../../domain/dates";
import { splitTodayItems } from "./todayUtils";
import { FocusLogger } from "./FocusLogger";

export function TodayPanel({
  items,
  onLogFocus,
}: {
  items: WorkspaceItem[];
  onLogFocus: (item: WorkspaceItem, minutes: number) => void;
}) {
  const groups = splitTodayItems(items, todayIsoDate());

  return (
    <aside className="today-panel">
      <p className="eyebrow">Today</p>
      <h2>{groups.today.length} planned</h2>
      <section>
        <h3>Scheduled</h3>
        {groups.today.map((item) => <p key={item.id}>{item.title}</p>)}
      </section>
      <section>
        <h3>Overdue</h3>
        {groups.overdue.map((item) => <p key={item.id}>{item.title}</p>)}
      </section>
      <FocusLogger items={items.filter((item) => item.status !== "Done")} onLog={onLogFocus} />
    </aside>
  );
}
```

- [ ] **Step 7: Wire Today panel into BoardPage**

Import `TodayPanel` and `logFocusSession`. Add a mutation that calls `logFocusSession` with `startedAt` as `new Date(Date.now() - minutes * 60_000).toISOString()` and `endedAt` as `new Date().toISOString()`. Render `TodayPanel` beside the board grid when `boardQuery.data` is loaded.

- [ ] **Step 8: Verify**

```powershell
npm test -- src/features/today/todayUtils.test.ts
npm run build
```

Expected: tests and build pass.

- [ ] **Step 9: Commit**

```powershell
git add personal-progress-workspace/src personal-progress-workspace/supabase/migrations/202605050001_initial_schema.sql
git commit -m "feat: add today panel focus logging"
```

---

### Task 9: Add Goals And Milestones

**Files:**
- Create: `personal-progress-workspace/src/features/goals/goalUtils.ts`
- Create: `personal-progress-workspace/src/features/goals/goalUtils.test.ts`
- Create: `personal-progress-workspace/src/features/goals/GoalsPage.tsx`
- Create: `personal-progress-workspace/src/features/goals/GoalDetail.tsx`
- Create: `personal-progress-workspace/src/features/goals/goalApi.ts`
- Modify: `personal-progress-workspace/src/App.tsx`

- [ ] **Step 1: Write failing goal utility test**

`src/features/goals/goalUtils.test.ts`:

```ts
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
```

- [ ] **Step 2: Create goal utilities**

`src/features/goals/goalUtils.ts`:

```ts
import type { GoalStatus } from "../../domain/types";

export function summarizeGoal(input: { progress: number; status: GoalStatus }) {
  const statusLabel = input.status.charAt(0).toUpperCase() + input.status.slice(1);
  return {
    progressLabel: `${Math.round(input.progress)}%`,
    statusLabel,
  };
}
```

- [ ] **Step 3: Create goal API**

`src/features/goals/goalApi.ts`:

```ts
import { supabase } from "../../lib/supabase";
import type { Goal, Milestone, WorkspaceItem } from "../../domain/types";

export interface GoalBundle {
  goals: Goal[];
  milestones: Milestone[];
  linkedItems: WorkspaceItem[];
}

export async function loadGoals(workspaceId: string): Promise<GoalBundle> {
  const [{ data: goals, error: goalsError }, { data: milestones, error: milestonesError }, { data: items, error: itemsError }] =
    await Promise.all([
      supabase.from("goals").select("*").eq("workspace_id", workspaceId).order("created_at", { ascending: false }),
      supabase.from("milestones").select("*").eq("workspace_id", workspaceId).order("sort_order", { ascending: true }),
      supabase.from("items").select("*").eq("workspace_id", workspaceId).not("goal_id", "is", null),
    ]);

  if (goalsError) throw goalsError;
  if (milestonesError) throw milestonesError;
  if (itemsError) throw itemsError;

  return {
    goals: (goals ?? []).map((row) => ({
      id: row.id,
      workspaceId: row.workspace_id,
      title: row.title,
      description: row.description,
      status: row.status,
      targetDate: row.target_date,
      progressMode: row.progress_mode,
      manualProgressValue: row.manual_progress_value,
    })),
    milestones: (milestones ?? []).map((row) => ({
      id: row.id,
      goalId: row.goal_id,
      title: row.title,
      status: row.status,
      dueDate: row.due_date,
      sortOrder: Number(row.sort_order),
    })),
    linkedItems: (items ?? []).map((row) => ({
      id: row.id,
      workspaceId: row.workspace_id,
      boardId: row.board_id,
      columnId: row.column_id,
      goalId: row.goal_id,
      title: row.title,
      description: row.description,
      type: row.type,
      tags: row.tags ?? [],
      priority: row.priority,
      status: row.status,
      scheduledDate: row.scheduled_date,
      dueDate: row.due_date,
      estimateMinutes: row.estimate_minutes,
      focusMinutesTotal: row.focus_minutes_total,
      progressMode: row.progress_mode,
      progressValue: row.progress_value,
      sortOrder: Number(row.sort_order),
    })),
  };
}
```

- [ ] **Step 4: Create Goals page**

`src/features/goals/GoalsPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { calculateGoalProgress } from "../../domain/progress";
import type { WorkspaceRecord } from "../workspaces/workspaceApi";
import { loadGoals } from "./goalApi";
import { summarizeGoal } from "./goalUtils";

export function GoalsPage({ workspace }: { workspace: WorkspaceRecord }) {
  const goalsQuery = useQuery({
    queryKey: ["goals", workspace.id],
    queryFn: () => loadGoals(workspace.id),
  });

  if (goalsQuery.isLoading) return <section className="board-state">Loading goals...</section>;
  if (goalsQuery.isError) return <section className="board-state">Goals failed to load.</section>;

  return (
    <section className="goals-page">
      <header>
        <p className="eyebrow">Goals</p>
        <h1>Progress dashboard</h1>
      </header>
      <div className="goal-grid">
        {goalsQuery.data.goals.map((goal) => {
          const milestones = goalsQuery.data.milestones.filter((milestone) => milestone.goalId === goal.id);
          const linkedItems = goalsQuery.data.linkedItems.filter((item) => item.goalId === goal.id);
          const progress = calculateGoalProgress({
            mode: goal.progressMode,
            milestones,
            linkedItems,
            focusMinutesTotal: linkedItems.reduce((sum, item) => sum + item.focusMinutesTotal, 0),
            focusTargetMinutes: linkedItems.reduce((sum, item) => sum + (item.estimateMinutes ?? 0), 0),
            manualProgressValue: goal.manualProgressValue,
          });
          const summary = summarizeGoal({ progress, status: goal.status });

          return (
            <article className="goal-card" key={goal.id}>
              <span>{summary.statusLabel}</span>
              <h2>{goal.title}</h2>
              <strong>{summary.progressLabel}</strong>
              <div className="progress-bar"><span style={{ width: `${progress}%` }} /></div>
            </article>
          );
        })}
      </div>
    </section>
  );
}
```

- [ ] **Step 5: Create `GoalDetail.tsx`**

```tsx
import type { Goal, Milestone, WorkspaceItem } from "../../domain/types";

export function GoalDetail({
  goal,
  milestones,
  linkedItems,
}: {
  goal: Goal;
  milestones: Milestone[];
  linkedItems: WorkspaceItem[];
}) {
  return (
    <section className="goal-detail">
      <h2>{goal.title}</h2>
      <h3>Milestones</h3>
      {milestones.map((milestone) => <p key={milestone.id}>{milestone.title}</p>)}
      <h3>Linked items</h3>
      {linkedItems.map((item) => <p key={item.id}>{item.title}</p>)}
    </section>
  );
}
```

- [ ] **Step 6: Add simple navigation in App**

Create local route state in `AppContent`:

```tsx
const [view, setView] = useState<"board" | "goals">("board");
```

Render two buttons and switch between `BoardPage` and `GoalsPage`. Import `useState` and `GoalsPage`.

- [ ] **Step 7: Verify**

```powershell
npm test -- src/features/goals/goalUtils.test.ts
npm run build
```

Expected: tests and build pass.

- [ ] **Step 8: Commit**

```powershell
git add personal-progress-workspace/src/features/goals personal-progress-workspace/src/App.tsx
git commit -m "feat: add goals dashboard"
```

---

### Task 10: Add Command Palette

**Files:**
- Create: `personal-progress-workspace/src/features/command/commandActions.ts`
- Create: `personal-progress-workspace/src/features/command/CommandPalette.tsx`
- Modify: `personal-progress-workspace/src/App.tsx`

- [ ] **Step 1: Create command actions**

`src/features/command/commandActions.ts`:

```ts
export type CommandAction =
  | { id: "create-item"; label: "Create item"; keywords: string[] }
  | { id: "open-board"; label: "Open board"; keywords: string[] }
  | { id: "open-goals"; label: "Open goals"; keywords: string[] }
  | { id: "log-focus"; label: "Log focus"; keywords: string[] };

export const commandActions: CommandAction[] = [
  { id: "create-item", label: "Create item", keywords: ["new", "task", "content", "learning"] },
  { id: "open-board", label: "Open board", keywords: ["workspace", "kanban"] },
  { id: "open-goals", label: "Open goals", keywords: ["milestone", "progress"] },
  { id: "log-focus", label: "Log focus", keywords: ["time", "deep work"] },
];
```

- [ ] **Step 2: Create command palette**

`src/features/command/CommandPalette.tsx`:

```tsx
import { Command } from "cmdk";
import { useEffect, useState } from "react";
import { commandActions, type CommandAction } from "./commandActions";

export function CommandPalette({ onAction }: { onAction: (action: CommandAction) => void }) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((value) => !value);
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!open) return null;

  return (
    <div className="command-overlay" role="presentation" onMouseDown={() => setOpen(false)}>
      <Command className="command-menu" onMouseDown={(event) => event.stopPropagation()}>
        <Command.Input placeholder="Run a command..." autoFocus />
        <Command.List>
          <Command.Empty>No command found.</Command.Empty>
          {commandActions.map((action) => (
            <Command.Item
              key={action.id}
              value={`${action.label} ${action.keywords.join(" ")}`}
              onSelect={() => {
                onAction(action);
                setOpen(false);
              }}
            >
              {action.label}
            </Command.Item>
          ))}
        </Command.List>
      </Command>
    </div>
  );
}
```

- [ ] **Step 3: Wire into `App.tsx`**

Render:

```tsx
<CommandPalette
  onAction={(action) => {
    if (action.id === "open-board") setView("board");
    if (action.id === "open-goals") setView("goals");
    if (action.id === "create-item") setView("board");
    if (action.id === "log-focus") setView("board");
  }}
/>
```

Import `CommandPalette`.

- [ ] **Step 4: Add command styles**

Append:

```css
.command-overlay {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: grid;
  place-items: start center;
  padding-top: 12vh;
  background: rgba(2, 6, 23, 0.58);
}

.command-menu {
  width: min(640px, calc(100vw - 32px));
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #101827;
  box-shadow: var(--shadow);
  overflow: hidden;
}

.command-menu input {
  width: 100%;
  padding: 18px;
  border: 0;
  border-bottom: 1px solid var(--line);
  background: transparent;
  color: var(--text);
  outline: none;
}

.command-menu [cmdk-item] {
  padding: 12px 18px;
  cursor: pointer;
}

.command-menu [cmdk-item][data-selected="true"] {
  background: rgba(56, 189, 248, 0.14);
}
```

- [ ] **Step 5: Verify**

```powershell
npm run build
```

Expected: build passes; `Ctrl+K` opens the command palette during local manual test.

- [ ] **Step 6: Commit**

```powershell
git add personal-progress-workspace/src/features/command personal-progress-workspace/src/App.tsx personal-progress-workspace/src/styles/app.css
git commit -m "feat: add command palette"
```

---

### Task 11: Add Playwright Smoke Test

**Files:**
- Create: `personal-progress-workspace/playwright.config.ts`
- Create: `personal-progress-workspace/tests/smoke.spec.ts`

- [ ] **Step 1: Create Playwright config**

`playwright.config.ts`:

```ts
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:5173",
    trace: "on-first-retry",
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1",
    url: "http://127.0.0.1:5173",
    reuseExistingServer: true,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
```

- [ ] **Step 2: Create smoke test for unauthenticated shell**

`tests/smoke.spec.ts`:

```ts
import { expect, test } from "@playwright/test";

test("shows sign in screen", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: /sign in to your command center/i })).toBeVisible();
});
```

- [ ] **Step 3: Run Playwright install**

```powershell
npx playwright install chromium
```

Expected: Chromium browser is installed or already present.

- [ ] **Step 4: Run e2e**

```powershell
npm run e2e
```

Expected: smoke test passes.

- [ ] **Step 5: Commit**

```powershell
git add personal-progress-workspace/playwright.config.ts personal-progress-workspace/tests
git commit -m "test: add workspace smoke test"
```

---

### Task 12: Final Verification And MVP Readiness Pass

**Files:**
- Modify: `personal-progress-workspace/README.md`

- [ ] **Step 1: Replace README**

`README.md`:

```md
# Personal Progress Workspace

Board-first personal progress workspace for creative work, learning, and personal-life planning.

## Stack

- Vite React TypeScript
- Supabase auth and Postgres
- React Query
- dnd-kit
- cmdk
- Vitest
- Playwright

## Setup

1. Create a Supabase project.
2. Run `supabase/migrations/202605050001_initial_schema.sql` in the Supabase SQL editor or through Supabase CLI.
3. Copy `.env.example` to `.env.local`.
4. Fill `VITE_SUPABASE_URL` and `VITE_SUPABASE_ANON_KEY`.
5. Run `npm install`.
6. Run `npm run dev`.

## Verification

Run:

```powershell
npm test
npm run build
npm run e2e
```
```

- [ ] **Step 2: Run full verification**

```powershell
npm test
npm run build
npm run e2e
```

Expected:

- Vitest passes.
- TypeScript and Vite build pass.
- Playwright smoke test passes.

- [ ] **Step 3: Inspect git status**

```powershell
git status --short
```

Expected: only intended app files are modified or staged. Do not stage `.superpowers/` visual companion files.

- [ ] **Step 4: Commit**

```powershell
git add personal-progress-workspace
git commit -m "docs: add progress workspace setup guide"
```

---

## Self-Review Checklist

- Spec coverage:
  - Board-first workspace: Tasks 5, 6, 7.
  - Supabase auth and cloud sync: Tasks 3, 4, 5.
  - Personal workspace with future collaboration foundation: Tasks 3, 4.
  - Today panel and focus logging: Task 8.
  - Goals, milestones, and progress: Tasks 2, 9.
  - Command palette: Task 10.
  - Rich dark UI: Tasks 1, 6, 10.
  - Testing: Tasks 2, 5, 8, 9, 11, 12.
  - Notifications excluded from MVP: Locked Decisions and README scope.
- Red-flag scan:
  - No unresolved markers or vague validation-only steps should remain.
- Type consistency:
  - Domain types use camelCase.
  - Supabase rows use snake_case.
  - API mapping functions convert snake_case to camelCase at boundaries.
