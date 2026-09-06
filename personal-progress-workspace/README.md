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
7. Open the app and use `Create account instead` on the auth screen for first-time setup.

## Deploy on a Free Vercel Subdomain

Use Vercel Hobby for a quick public test URL such as `your-project.vercel.app`.

1. Push this repository branch to GitHub.
2. In Vercel, create a new project from the GitHub repository.
3. Use these project settings:
   - Framework preset: `Vite`
   - Install command: `npm ci`
   - Build command: `npm run build`
   - Output directory: `dist`
4. Add these Vercel environment variables for Production, Preview, and Development:
   - `VITE_SUPABASE_URL`
   - `VITE_SUPABASE_ANON_KEY`
5. Deploy the project.
6. Copy the generated `https://<project-name>.vercel.app` URL.
7. Add that URL to Supabase Auth settings before sharing it with testers.

If using the Vercel CLI after logging in locally, run:

```powershell
npm install -g vercel
vercel
vercel --prod
```

The first `vercel` command links the local project and creates a preview deployment. The `vercel --prod` command publishes the production URL.

## Supabase Production Checklist

Before sharing the public URL:

1. Run `supabase/migrations/202605050001_initial_schema.sql` in the Supabase SQL editor or through Supabase CLI.
2. Confirm Row Level Security is enabled for:
   - `workspaces`
   - `workspace_members`
   - `boards`
   - `board_columns`
   - `goals`
   - `items`
   - `milestones`
   - `focus_sessions`
   - `habit_logs`
   - `activity_events`
3. In Supabase Auth URL Configuration, set Site URL to the production Vercel URL.
4. Add the production Vercel URL to Redirect URLs.
5. Add any preview URLs only while actively testing previews.
6. Keep only `VITE_SUPABASE_URL` and `VITE_SUPABASE_ANON_KEY` in frontend hosting env vars.
7. Do not expose the Supabase service role key in Vercel or frontend code.
8. Enable email confirmations if you want users to prove ownership of their email before using the workspace.
9. Review Supabase email rate limits before inviting more than a small test group.
10. Create a test account from the deployed URL and confirm workspace data is isolated from another test account.

## Verification

Run:

```powershell
npm test
npm run build
npm run e2e
```
