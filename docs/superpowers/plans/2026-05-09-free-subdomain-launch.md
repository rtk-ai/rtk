# Free Subdomain Launch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare the app for a small public test on a Vercel free subdomain with deployment docs, security headers, and a Supabase production checklist.

**Architecture:** Keep the app as a static Vite React build deployed to Vercel. Add a root `vercel.json` to define routing/build metadata and baseline security headers, and expand the app README with deployment and Supabase production steps.

**Tech Stack:** Vite, React, TypeScript, Supabase, Vercel, Markdown docs, Vitest, Playwright.

---

### Task 1: Add Vercel Static App Configuration

**Files:**
- Create: `personal-progress-workspace/vercel.json`

- [ ] **Step 1: Create Vercel config**

Create `personal-progress-workspace/vercel.json` with this exact JSON:

```json
{
  "buildCommand": "npm run build",
  "installCommand": "npm ci",
  "outputDirectory": "dist",
  "headers": [
    {
      "source": "/(.*)",
      "headers": [
        {
          "key": "Content-Security-Policy",
          "value": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; font-src 'self' data:; connect-src 'self' https://*.supabase.co wss://*.supabase.co; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"
        },
        {
          "key": "X-Content-Type-Options",
          "value": "nosniff"
        },
        {
          "key": "Referrer-Policy",
          "value": "strict-origin-when-cross-origin"
        },
        {
          "key": "Permissions-Policy",
          "value": "camera=(), microphone=(), geolocation=(), payment=()"
        }
      ]
    }
  ],
  "rewrites": [
    {
      "source": "/(.*)",
      "destination": "/index.html"
    }
  ]
}
```

- [ ] **Step 2: Validate JSON syntax**

Run:

```powershell
node -e "JSON.parse(require('fs').readFileSync('vercel.json','utf8')); console.log('vercel.json valid')"
```

Expected output:

```text
vercel.json valid
```

- [ ] **Step 3: Commit Vercel config**

Run from repo root:

```powershell
git add personal-progress-workspace/vercel.json
git commit -m "chore: add Vercel deployment config"
```

Expected: commit succeeds and includes only `personal-progress-workspace/vercel.json`.

---

### Task 2: Document Free Subdomain Deployment

**Files:**
- Modify: `personal-progress-workspace/README.md`

- [ ] **Step 1: Add deployment section**

Edit `personal-progress-workspace/README.md` after the `Setup` section and before `Verification` to include:

```markdown
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
```

- [ ] **Step 2: Verify deployment docs mention required Vercel settings**

Run:

```powershell
Select-String -Path README.md -Pattern 'Deploy on a Free Vercel Subdomain','npm ci','npm run build','dist','VITE_SUPABASE_URL','VITE_SUPABASE_ANON_KEY'
```

Expected: output contains one match for each listed pattern.

- [ ] **Step 3: Commit deployment docs**

Run from repo root:

```powershell
git add personal-progress-workspace/README.md
git commit -m "docs: add Vercel free subdomain deploy guide"
```

Expected: commit succeeds and includes only `personal-progress-workspace/README.md`.

---

### Task 3: Document Supabase Production Checklist

**Files:**
- Modify: `personal-progress-workspace/README.md`

- [ ] **Step 1: Add Supabase production checklist**

Edit `personal-progress-workspace/README.md` after the Vercel deployment section and before `Verification` to include:

```markdown
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
```

- [ ] **Step 2: Verify checklist includes security-critical items**

Run:

```powershell
Select-String -Path README.md -Pattern 'Row Level Security','Site URL','Redirect URLs','service role key','email confirmations','data is isolated'
```

Expected: output contains one match for each listed pattern.

- [ ] **Step 3: Commit Supabase checklist**

Run from repo root:

```powershell
git add personal-progress-workspace/README.md
git commit -m "docs: add Supabase production checklist"
```

Expected: commit succeeds and includes only `personal-progress-workspace/README.md`.

---

### Task 4: Final Verification

**Files:**
- Verify: `personal-progress-workspace/vercel.json`
- Verify: `personal-progress-workspace/README.md`

- [ ] **Step 1: Run unit test suite**

Run:

```powershell
npm test
```

Expected: all Vitest test files pass.

- [ ] **Step 2: Run production build**

Run:

```powershell
npm run build
```

Expected: TypeScript build and Vite production build complete successfully.

- [ ] **Step 3: Run smoke e2e test**

Run:

```powershell
npm run e2e
```

Expected: Playwright smoke test passes.

- [ ] **Step 4: Inspect final git diff**

Run:

```powershell
git status --short
git log --oneline -5
```

Expected: working tree is clean after commits, and recent commits include the Vercel config and README deploy/checklist docs.
