# Free Subdomain Launch Design

## Goal

Prepare Personal Progress Workspace for a small public test using a free hosted subdomain, with deployment instructions and baseline production safety checks.

The initial target is Vercel Hobby with a free `*.vercel.app` subdomain. The app remains a Vite React single-page app backed by the user's Supabase project.

## In Scope

- Add Vercel deployment configuration for a static Vite app.
- Add security headers suitable for a browser app using Supabase.
- Add README deployment steps for Vercel free subdomain publishing.
- Add a Supabase production checklist covering Auth, redirect URLs, env vars, and RLS.
- Keep the existing login/create-account UI copy unchanged.

## Out of Scope

- No beta access gate.
- No custom paid domain setup.
- No domain purchase or DNS automation.
- No auth UI wording changes.
- No paid hosting requirements.

## Deployment Design

Use Vercel as the first public hosting target because it supports Vite builds directly, provides HTTPS automatically, and creates a free `*.vercel.app` subdomain.

Expected Vercel settings:

- Framework preset: Vite
- Build command: `npm run build`
- Output directory: `dist`
- Install command: `npm ci`
- Environment variables:
  - `VITE_SUPABASE_URL`
  - `VITE_SUPABASE_ANON_KEY`

The exact public subdomain is created by Vercel during deployment and can be renamed inside the Vercel project settings if the preferred name is available.

## Security Header Design

Add `vercel.json` with baseline response headers for all routes:

- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `Permissions-Policy` disabling camera, microphone, geolocation, and payment by default.
- `Content-Security-Policy` that permits the app to load itself and connect to Supabase over HTTPS.

The Content Security Policy should remain compatible with Vite's production bundle and Supabase Auth. It should not include development-only allowances unless required by local dev, because `vercel.json` only applies to the deployed app.

## Supabase Production Checklist

Document the required Supabase settings before sharing the public URL:

- Run the migration in `supabase/migrations/202605050001_initial_schema.sql`.
- Keep Row Level Security enabled on all workspace data tables.
- Use only the public anon key in Vercel frontend env vars; never expose a service role key.
- Set Supabase Auth Site URL to the Vercel production URL.
- Add Vercel production URL to Supabase Auth Redirect URLs.
- Enable email confirmation if account ownership should be verified before access.
- Review email rate limits before inviting more than a small test group.

## Error Handling

If Vercel env vars are missing, the existing auth screen should continue to show the current Supabase setup message and avoid sending auth requests.

If Supabase Auth redirect URLs are wrong, sign-in or email confirmation may fail after redirect. The README checklist should call this out explicitly.

## Testing

After implementation:

- Run `npm test`.
- Run `npm run build`.
- Run `npm run e2e`.
- Verify `vercel.json` is valid JSON.
- Confirm README contains the deploy steps and Supabase checklist.

## Open Requirements

The user will need to log in to Vercel and Supabase in their own accounts. Codex can prepare the repository and can run deployment commands only if the local machine is already authenticated or the user completes the login flow.
