import { Home, LayoutDashboard, Target } from "lucide-react";
import { useState } from "react";
import { AuthProvider, useAuth } from "./features/auth/AuthProvider";
import { SignInPage } from "./features/auth/SignInPage";
import { BoardPage } from "./features/boards/BoardPage";
import { CommandPalette } from "./features/command/CommandPalette";
import type { CommandAction } from "./features/command/commandActions";
import { DashboardPage } from "./features/dashboard/DashboardPage";
import { GoalsPage } from "./features/goals/GoalsPage";
import { useWorkspace } from "./features/workspaces/useWorkspace";

type WorkspaceView = "dashboard" | "board" | "goals";

function AppContent() {
  const { user, loading: authLoading } = useAuth();
  const [view, setView] = useState<WorkspaceView>("dashboard");
  const workspace = useWorkspace();

  if (authLoading) return <main className="app-shell">Loading session...</main>;
  if (!user) return <SignInPage />;
  if (workspace.isLoading) return <main className="app-shell">Preparing workspace...</main>;
  if (workspace.isError || !workspace.data) {
    return (
      <main className="app-shell">
        <section className="hero-panel" role="alert">
          <p className="eyebrow">Workspace setup</p>
          <h1>Workspace failed to load</h1>
          <p>{getWorkspaceErrorMessage(workspace.error)}</p>
          <button className="app-view-nav__button" type="button" onClick={() => void workspace.refetch()}>
            Retry workspace load
          </button>
        </section>
      </main>
    );
  }

  function handleCommand(action: CommandAction) {
    if (action.id === "open-dashboard") {
      setView("dashboard");
    }

    if (action.id === "open-board" || action.id === "create-item" || action.id === "log-focus") {
      setView("board");
    }

    if (action.id === "open-goals") {
      setView("goals");
    }
  }

  return (
    <>
      <nav className="app-view-nav" aria-label="Workspace views">
        <button
          className={`app-view-nav__button ${view === "dashboard" ? "app-view-nav__button--active" : ""}`}
          type="button"
          onClick={() => setView("dashboard")}
        >
          <Home size={16} aria-hidden="true" />
          Dashboard
        </button>
        <button
          className={`app-view-nav__button ${view === "board" ? "app-view-nav__button--active" : ""}`}
          type="button"
          onClick={() => setView("board")}
        >
          <LayoutDashboard size={16} aria-hidden="true" />
          Board
        </button>
        <button
          className={`app-view-nav__button ${view === "goals" ? "app-view-nav__button--active" : ""}`}
          type="button"
          onClick={() => setView("goals")}
        >
          <Target size={16} aria-hidden="true" />
          Goals
        </button>
      </nav>
      <CommandPalette onAction={handleCommand} />
      {view === "dashboard" ? (
        <DashboardPage
          workspace={workspace.data}
          onOpenBoard={() => setView("board")}
          onOpenGoals={() => setView("goals")}
        />
      ) : null}
      {view === "board" ? <BoardPage workspace={workspace.data} /> : null}
      {view === "goals" ? <GoalsPage workspace={workspace.data} /> : null}
    </>
  );
}

export function App() {
  return (
    <AuthProvider>
      <AppContent />
    </AuthProvider>
  );
}

function getWorkspaceErrorMessage(error: unknown) {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") {
    return error.message;
  }

  return "No workspace data was returned. Check Supabase Auth, RLS policies, and the workspace migration.";
}
