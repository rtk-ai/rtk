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
  updated_at timestamptz not null default now(),
  unique (owner_user_id)
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
  updated_at timestamptz not null default now(),
  unique (id, workspace_id)
);

create table public.board_columns (
  id uuid primary key default gen_random_uuid(),
  board_id uuid not null references public.boards(id) on delete cascade,
  name text not null,
  status_key text not null check (status_key in ('Inbox', 'Planned', 'Doing', 'Review', 'Done')),
  sort_order numeric not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (board_id, status_key),
  unique (id, board_id, status_key)
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
  updated_at timestamptz not null default now(),
  unique (id, workspace_id)
);

create table public.items (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  board_id uuid not null,
  column_id uuid not null,
  goal_id uuid,
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
  updated_at timestamptz not null default now(),
  unique (id, workspace_id),
  foreign key (board_id, workspace_id) references public.boards(id, workspace_id) on delete cascade,
  foreign key (column_id, board_id, status) references public.board_columns(id, board_id, status_key) on delete restrict,
  foreign key (goal_id, workspace_id) references public.goals(id, workspace_id) on delete set null (goal_id)
);

create table public.milestones (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  goal_id uuid not null,
  title text not null,
  status text not null default 'active' check (status in ('active', 'done')),
  due_date date,
  sort_order numeric not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  foreign key (goal_id, workspace_id) references public.goals(id, workspace_id) on delete cascade
);

create table public.focus_sessions (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  item_id uuid,
  goal_id uuid,
  started_at timestamptz not null,
  ended_at timestamptz not null,
  duration_minutes integer not null check (duration_minutes >= 0),
  notes text,
  created_at timestamptz not null default now(),
  check (ended_at >= started_at),
  foreign key (item_id, workspace_id) references public.items(id, workspace_id) on delete set null (item_id),
  foreign key (goal_id, workspace_id) references public.goals(id, workspace_id) on delete set null (goal_id)
);

create table public.habit_logs (
  id uuid primary key default gen_random_uuid(),
  workspace_id uuid not null references public.workspaces(id) on delete cascade,
  item_id uuid,
  goal_id uuid,
  log_date date not null,
  value integer not null default 1,
  created_at timestamptz not null default now(),
  unique (workspace_id, item_id, log_date),
  foreign key (item_id, workspace_id) references public.items(id, workspace_id) on delete cascade,
  foreign key (goal_id, workspace_id) references public.goals(id, workspace_id) on delete set null (goal_id)
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
create index items_goal_id_idx on public.items(goal_id);
create index items_tags_idx on public.items using gin(tags);
create index goals_workspace_id_idx on public.goals(workspace_id);
create index milestones_goal_id_idx on public.milestones(goal_id);
create index focus_sessions_workspace_started_idx on public.focus_sessions(workspace_id, started_at desc);
create index focus_sessions_item_id_idx on public.focus_sessions(item_id);
create index focus_sessions_goal_id_idx on public.focus_sessions(goal_id);
create index habit_logs_workspace_date_idx on public.habit_logs(workspace_id, log_date desc);
create index habit_logs_item_id_idx on public.habit_logs(item_id);
create index habit_logs_goal_id_idx on public.habit_logs(goal_id);
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

create or replace function public.is_workspace_owner(target_workspace_id uuid)
returns boolean
language sql
security definer
set search_path = public
as $$
  select exists (
    select 1
    from public.workspaces w
    where w.id = target_workspace_id
      and w.owner_user_id = auth.uid()
  );
$$;

create or replace function public.update_item_positions(position_updates jsonb)
returns void
language plpgsql
set search_path = public
as $$
declare
  position_update record;
begin
  for position_update in
    select *
    from jsonb_to_recordset(position_updates) as update_row(
      id uuid,
      workspace_id uuid,
      board_id uuid,
      column_id uuid,
      status text,
      sort_order numeric
    )
  loop
    update public.items
    set column_id = position_update.column_id,
        status = position_update.status,
        sort_order = position_update.sort_order
    where id = position_update.id
      and workspace_id = position_update.workspace_id
      and board_id = position_update.board_id;

    if not found then
      raise exception 'item position update failed for item %', position_update.id
        using errcode = 'P0001';
    end if;
  end loop;
end;
$$;

create policy "workspace owners can create workspace"
on public.workspaces for insert
with check (owner_user_id = auth.uid());

create policy "workspace members can read workspace"
on public.workspaces for select
using (public.is_workspace_member(id));

create policy "workspace owners can read owned workspace"
on public.workspaces for select
using (owner_user_id = auth.uid());

create policy "workspace owners can update workspace"
on public.workspaces for update
using (owner_user_id = auth.uid())
with check (owner_user_id = auth.uid());

create policy "workspace owners can create owner membership"
on public.workspace_members for insert
with check (
  user_id = auth.uid()
  and role = 'owner'
  and public.is_workspace_owner(workspace_id)
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

create policy "members can read items"
on public.items for select
using (public.is_workspace_member(workspace_id));

create policy "members can create own items"
on public.items for insert
with check (
  public.is_workspace_member(workspace_id)
  and created_by = auth.uid()
);

create policy "members can update own items"
on public.items for update
using (
  public.is_workspace_member(workspace_id)
  and created_by = auth.uid()
)
with check (
  public.is_workspace_member(workspace_id)
  and created_by = auth.uid()
);

create policy "members can delete items"
on public.items for delete
using (public.is_workspace_member(workspace_id));

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

create policy "members can read activity events"
on public.activity_events for select
using (public.is_workspace_member(workspace_id));

create policy "members can create own activity events"
on public.activity_events for insert
with check (
  public.is_workspace_member(workspace_id)
  and actor_user_id = auth.uid()
);

create policy "members can delete activity events"
on public.activity_events for delete
using (public.is_workspace_member(workspace_id));

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
