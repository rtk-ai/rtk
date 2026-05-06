# Personal Progress Workspace

Board-first personal progress workspace for creative work, learning, and personal-life planning.

Notifications are intentionally outside the MVP scope.

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
