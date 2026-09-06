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

grant execute on function public.is_workspace_owner(uuid) to authenticated;

drop policy if exists "workspace owners can create owner membership"
on public.workspace_members;

create policy "workspace owners can create owner membership"
on public.workspace_members for insert
to authenticated
with check (
  user_id = auth.uid()
  and role = 'owner'
  and public.is_workspace_owner(workspace_id)
);

create or replace function public.create_owner_membership_for_workspace()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  insert into public.workspace_members (workspace_id, user_id, role)
  values (new.id, new.owner_user_id, 'owner')
  on conflict (workspace_id, user_id) do nothing;

  return new;
end;
$$;

drop trigger if exists workspaces_owner_membership
on public.workspaces;

create trigger workspaces_owner_membership
after insert on public.workspaces
for each row execute function public.create_owner_membership_for_workspace();

insert into public.workspace_members (workspace_id, user_id, role)
select id, owner_user_id, 'owner'
from public.workspaces
on conflict (workspace_id, user_id) do nothing;
