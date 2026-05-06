import { useState } from "react";
import type { FormEvent } from "react";
import { LogIn } from "lucide-react";
import { env } from "../../lib/env";
import { supabase } from "../../lib/supabase";

const missingSupabaseConfigError = "Supabase configuration is missing.";

export function SignInPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);

    if (!env.hasSupabaseConfig) {
      setError(missingSupabaseConfigError);
      return;
    }

    setSubmitting(true);

    try {
      const { error: signInError } = await supabase.auth.signInWithPassword({ email, password });

      if (signInError) setError(signInError.message);
    } catch (signInError) {
      setError(signInError instanceof Error ? signInError.message : "Sign in failed.");
    } finally {
      setSubmitting(false);
    }
  }

  const displayedError = error ?? (!env.hasSupabaseConfig ? missingSupabaseConfigError : null);

  return (
    <main className="auth-page">
      <form className="auth-card" onSubmit={handleSubmit}>
        <p className="eyebrow">Personal Progress Workspace</p>
        <h1>Sign in to your command center</h1>
        {!env.hasSupabaseConfig ? <p className="form-note">Supabase env vars are not configured locally.</p> : null}
        <label>
          Email
          <input value={email} onChange={(event) => setEmail(event.target.value)} type="email" required />
        </label>
        <label>
          Password
          <input value={password} onChange={(event) => setPassword(event.target.value)} type="password" required />
        </label>
        {displayedError ? <p className="form-error">{displayedError}</p> : null}
        <button type="submit" disabled={!env.hasSupabaseConfig || submitting}>
          <LogIn size={18} />
          {submitting ? "Signing in" : "Sign in"}
        </button>
      </form>
    </main>
  );
}
