import { useState } from "react";
import type { FormEvent } from "react";
import { BarChart3, LogIn, UserPlus } from "lucide-react";
import { env } from "../../lib/env";
import { supabase } from "../../lib/supabase";

const missingSupabaseConfigError = "Supabase configuration is missing.";
const accountCreatedMessage = "Account created. Check your email to confirm it, then sign in.";

type AuthMode = "sign-in" | "create-account";

export function SignInPage() {
  const [authOpen, setAuthOpen] = useState(false);
  const [mode, setMode] = useState<AuthMode>("sign-in");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const isCreatingAccount = mode === "create-account";

  function openAuth(nextMode: AuthMode) {
    setAuthOpen(true);
    switchMode(nextMode);
  }

  function switchMode(nextMode: AuthMode) {
    setMode(nextMode);
    setError(null);
    setNotice(null);
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    setNotice(null);

    if (!env.hasSupabaseConfig) {
      setError(missingSupabaseConfigError);
      return;
    }

    setSubmitting(true);

    try {
      if (isCreatingAccount) {
        const { error: signUpError } = await supabase.auth.signUp({ email, password });

        if (signUpError) {
          setError(signUpError.message);
        } else {
          setNotice(accountCreatedMessage);
        }

        return;
      }

      const { error: signInError } = await supabase.auth.signInWithPassword({ email, password });

      if (signInError) setError(signInError.message);
    } catch (signInError) {
      setError(signInError instanceof Error ? signInError.message : "Authentication failed.");
    } finally {
      setSubmitting(false);
    }
  }

  const displayedError = error ?? (!env.hasSupabaseConfig ? missingSupabaseConfigError : null);

  return (
    <main className="auth-page">
      <header className="auth-topbar">
        <div className="auth-brand">
          <BarChart3 size={18} aria-hidden="true" />
          Personal Progress Workspace
        </div>
        {!authOpen ? (
          <div className="auth-topbar__actions">
            <button className="auth-topbar__login" type="button" onClick={() => openAuth("sign-in")}>
              <LogIn size={16} aria-hidden="true" />
              Log in
            </button>
            <button
              className="auth-topbar__login auth-topbar__login--primary"
              type="button"
              onClick={() => openAuth("create-account")}
            >
              <UserPlus size={16} aria-hidden="true" />
              Create account
            </button>
          </div>
        ) : null}
      </header>

      <section className="auth-landing" aria-label="Personal progress workspace preview">
        <div>
          <p className="eyebrow">Progress command center</p>
          <h1>Track your work without losing the thread.</h1>
          <p>
            A focused dashboard for goals, today plans, overdue work, and deep-work progress once you sign in.
          </p>
        </div>
        <div className="auth-preview" aria-hidden="true">
          <span>Dashboard</span>
          <strong>2 open</strong>
          <div className="progress-bar">
            <span style={{ width: "68%" }} />
          </div>
          <p>Goals, board, focus, and priorities in one place.</p>
        </div>
      </section>

      {authOpen ? (
        <form className="auth-card" onSubmit={handleSubmit}>
          <p className="eyebrow">Personal Progress Workspace</p>
          <h1>{isCreatingAccount ? "Create your command center" : "Sign in to your command center"}</h1>
          <p className="auth-card__subtitle">
            {isCreatingAccount
              ? "Create an account once, then your workspace can sync through Supabase."
              : "Use your existing account to continue your workspace."}
          </p>
          {!env.hasSupabaseConfig ? <p className="form-note">Supabase env vars are not configured locally.</p> : null}
          <label>
            Email
            <input value={email} onChange={(event) => setEmail(event.target.value)} type="email" required />
          </label>
          <label>
            Password
            <input value={password} onChange={(event) => setPassword(event.target.value)} type="password" required />
          </label>
          {notice ? <p className="form-success">{notice}</p> : null}
          {displayedError ? <p className="form-error">{displayedError}</p> : null}
          <button type="submit" disabled={!env.hasSupabaseConfig || submitting}>
            {isCreatingAccount ? <UserPlus size={18} /> : <LogIn size={18} />}
            {submitting ? "Working" : isCreatingAccount ? "Create account" : "Sign in"}
          </button>
          <button
            className="auth-card__switch"
            type="button"
            disabled={submitting}
            onClick={() => switchMode(isCreatingAccount ? "sign-in" : "create-account")}
          >
            {isCreatingAccount ? "Sign in instead" : "Create account instead"}
          </button>
        </form>
      ) : null}
    </main>
  );
}
