# Personal Progress Workspace Design

Date: 2026-05-05
Status: Proposed
Owner: Codex

## Summary

Build a browser-based personal progress workspace for planning, tracking, and updating creative work, learning goals, and personal-life tasks. The app is a cloud-synced web application backed by Supabase. It uses a rich dark "productivity command center" visual direction, but its product structure is board-first: the main workspace is a drag-and-drop board for active work, with Today, Goals, and focus logging layered around it.

The initial release targets one personal user, while the data model includes workspace ownership and membership so collaboration can be added later without replacing the core schema.

## Goals

- Make it easy to update work throughout the day with drag-and-drop and command palette actions.
- Support creative/content work, learning/skill-building, and personal-life tasks as first-class item types.
- Track progress through tasks completed, milestones, focus time, and streaks.
- Provide cloud sync across browsers and machines through Supabase.
- Deliver an aesthetic, information-rich dark UI that is engaging without becoming corporate CRM software.
- Keep the MVP focused enough to ship before adding collaboration, notifications, AI parsing, or advanced calendar workflows.

## Non-Goals

- Native Windows, macOS, iOS, or Android apps in the MVP.
- Multi-user collaboration UI, invites, comments, or permissions management in the MVP.
- Phone, browser, email, or desktop notifications in the MVP.
- AI natural-language task parsing in the MVP.
- Offline-first conflict resolution in the MVP.
- Full monthly calendar planning in the MVP.
- Time tracking integrations with external apps in the MVP.

## Target Workflow

The user opens the app in a desktop browser as their daily workspace.

In the morning, they review the board and pull the most relevant items into the Today panel. During the day, they update work by dragging items between statuses or opening the command palette to create an item, change status, schedule something for today, log focus time, or find an item quickly.

Longer-term goals collect progress from milestones, linked board items, focus sessions, and habit logs. Goals are important, but the user should not have to start from a goals dashboard every day; the board remains the main surface.

## Product Approach

The selected approach is **Board-First Workspace**.

The workspace centers on kanban-style boards. The default columns are:

- `Inbox`
- `Planned`
- `Doing`
- `Review`
- `Done`

Boards are flexible enough to represent content pipelines, learning projects, personal routines, and general tasks. The Today panel and Goals dashboard are supporting layers:

- Today answers "what matters now?"
- Boards answer "where is each piece of work?"
- Goals answer "what larger outcome is this moving?"

This direction is stronger for the user's content and learning workflow than a pure daily planner or pure goals dashboard, while still allowing a daily command-center feel.

## Core Screens

### Workspace Board

The Workspace Board is the default screen after sign-in.

Key capabilities:

- Sidebar with workspace and board/project navigation.
- Board columns for status-based work.
- Drag-and-drop item movement between columns.
- Item cards with type, priority, due/scheduled date, goal link, focus estimate, and compact progress signal.
- Filters by type, tag, priority, due date, and goal.
- Quick inline create in a column.
- Detail drawer for editing an item without leaving the board.

### Today Panel

The Today Panel is always reachable from the board, either as a right panel or drawer.

Key capabilities:

- Items scheduled for today.
- Overdue items.
- Focus session start/stop and manual focus logging.
- Quick actions for "schedule today", "move to doing", "mark done", and "log focus".
- Daily progress summary based on completed items, focus minutes, and streak/habit status.

The Today Panel may become its own route later, but MVP should keep it connected to the board so daily planning does not fragment the workflow.

### Goals

The Goals screen provides a higher-level progress view.

Key capabilities:

- Goal list grouped by active, paused, and completed.
- Goal detail with milestones, linked board items, focus sessions, habit logs, and progress.
- Progress modes:
  - task completion
  - milestone completion
  - focus minutes
  - streak or habit completion
- Manual progress override for cases where an exact computed value is not appropriate.

### Command Palette

The Command Palette is the fast control layer.

It should open via keyboard shortcut and support:

- Create item.
- Search items, boards, and goals.
- Change item status.
- Schedule item for today.
- Log focus minutes.
- Start focus session.
- Create milestone.
- Jump to board or goal.

Natural-language parsing is not part of MVP. The palette should use explicit commands and structured fields.

## Visual Direction

The selected visual style is **Rich Productivity Dashboard**.

Principles:

- Dark foundation with strong contrast and clear hierarchy.
- Multiple status colors, but avoid a single dominant hue theme.
- Dense but organized information, designed for scanning and repeated use.
- Progress rings, compact charts, badges, and bars where they clarify state.
- Cards are used for individual items and panels, not nested decorative layouts.
- Desktop browser is the primary experience. Mobile should remain usable but is not deeply optimized in MVP.

The product should feel like a creative operations command center, not a corporate CRM and not a minimalist todo list.

## Data Model

Supabase Postgres is the source of truth.

### Tables

`workspaces`

- `id`
- `name`
- `owner_user_id`
- `created_at`
- `updated_at`

`workspace_members`

- `workspace_id`
- `user_id`
- `role`
- `created_at`

Roles can start with `owner` and `member`, even if MVP only creates an owner membership.

`boards`

- `id`
- `workspace_id`
- `name`
- `description`
- `sort_order`
- `created_at`
- `updated_at`

`board_columns`

- `id`
- `board_id`
- `name`
- `status_key`
- `sort_order`
- `created_at`
- `updated_at`

`items`

- `id`
- `workspace_id`
- `board_id`
- `column_id`
- `goal_id`
- `title`
- `description`
- `type`
- `tags`
- `priority`
- `status`
- `scheduled_date`
- `due_date`
- `estimate_minutes`
- `focus_minutes_total`
- `progress_mode`
- `progress_value`
- `sort_order`
- `created_by`
- `created_at`
- `updated_at`

Item types:

- `task`
- `content`
- `learning`
- `habit`
- `personal`

Progress modes:

- `tasks`
- `milestones`
- `focus_time`
- `streak`
- `manual`

`goals`

- `id`
- `workspace_id`
- `title`
- `description`
- `status`
- `target_date`
- `progress_mode`
- `manual_progress_value`
- `created_at`
- `updated_at`

`milestones`

- `id`
- `workspace_id`
- `goal_id`
- `title`
- `status`
- `due_date`
- `sort_order`
- `created_at`
- `updated_at`

`focus_sessions`

- `id`
- `workspace_id`
- `item_id`
- `goal_id`
- `started_at`
- `ended_at`
- `duration_minutes`
- `notes`
- `created_at`

`habit_logs`

- `id`
- `workspace_id`
- `item_id`
- `goal_id`
- `log_date`
- `value`
- `created_at`

`activity_events`

- `id`
- `workspace_id`
- `actor_user_id`
- `entity_type`
- `entity_id`
- `event_type`
- `metadata`
- `created_at`

Activity events support audit history and provide a foundation for future in-app notifications, but notifications themselves are not part of MVP.

Tags are stored as a text array on `items` in MVP. They can be normalized into `tags` and `item_tags` tables later if tag management becomes complex.

### Row Level Security

RLS should allow users to read and mutate rows only when they are members of the associated workspace.

MVP creates one personal workspace automatically after first sign-in. Future collaboration can add invited users to `workspace_members` without changing ownership boundaries.

## Sync Strategy

Supabase client handles authenticated reads and writes. Realtime can be enabled for item and board updates, but MVP only needs straightforward cloud sync across browsers and machines.

Expected behavior:

- After sign-in, load the user's workspace, boards, goals, and active items.
- Mutations write directly to Supabase.
- The UI should optimistically update common interactions such as drag-and-drop, then reconcile with the persisted result.
- If a mutation fails, show an inline error and restore the previous state for that item.

Advanced conflict resolution is out of scope for MVP.

## Error Handling

- Auth errors should route the user back to sign-in with a short explanation.
- Supabase read/write failures should show a non-blocking toast and keep the user on the current screen.
- Drag-and-drop failures should revert the moved card and display a message.
- Missing workspace after sign-in should trigger workspace creation once, then retry loading.
- Empty states should be useful and action-oriented: create board, create item, create goal.

## MVP Scope

MVP includes:

- Supabase auth.
- Automatic personal workspace creation.
- Board-first workspace with CRUD for boards, columns, and items.
- Drag-and-drop item movement.
- Filters by type, priority, tag, due date, and goal.
- Item detail drawer.
- Today panel with scheduled items, overdue items, and focus logging.
- Goals screen with milestones and computed progress.
- Command palette for create, search, status updates, scheduling, focus logging, and navigation.
- Rich dark dashboard UI optimized for desktop browser.

MVP excludes:

- Collaboration UI and invites.
- Notifications.
- PWA install flow and phone push notifications.
- Full calendar month/week view.
- AI natural-language parsing.
- Native app packaging.
- Offline-first mode.

## Testing Strategy

Unit tests:

- Progress calculation by progress mode.
- Item filtering and sorting.
- Focus duration calculation.
- Workspace access helper behavior.

Component tests:

- Board rendering and card status changes.
- Item detail drawer save/cancel states.
- Today panel scheduling and focus logging.
- Command palette command routing.
- Goals progress display.

End-to-end smoke tests:

- Sign in with a test user or local mock.
- Create a board item.
- Drag the item to another column.
- Schedule the item for today.
- Log a focus session.
- Create a goal and milestone.
- Link an item to a goal and verify progress changes.

Visual QA:

- Desktop board layout at common laptop and large monitor widths.
- Today panel open/closed states.
- Command palette overlay.
- Empty states for new user data.

## Implementation Notes

The likely frontend direction is a TypeScript React web app. Supabase provides auth and database access. A drag-and-drop library should be used instead of hand-rolling pointer logic. The command palette should use an accessible dialog/combobox pattern rather than a custom unmanaged overlay.

The app can live as a separate project directory in this repository unless a new repository is chosen later.

## Open Decisions

- Exact frontend scaffold: Vite React, Next.js, or another React setup.
- Styling system: CSS modules, Tailwind, or component-library-assisted styling.
- Drag-and-drop library selection.
- Whether the first implementation uses Supabase local development or a hosted Supabase project from the start.
