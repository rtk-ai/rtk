import { LayoutDashboard, Target } from "lucide-react";
import { useState } from "react";
import { AuthProvider, useAuth } from "./features/auth/AuthProvider";
import { SignInPage } from "./features/auth/SignInPage";
import { BoardPage } from "./features/boards/BoardPage";
import { CommandPalette } from "./features/command/CommandPalette";
import type { CommandAction } from "./features/command/commandActions";
import { GoalsPage } from "./features/goals/GoalsPage";
import { useWorkspace } from "./features/workspaces/useWorkspace";

type WorkspaceView = "board" | "goals";

function AppContent() {
  const { user, loading: authLoading } = useAuth();
  const [view, setView] = useState<WorkspaceView>("board");
  const workspace = useWorkspace();

  if (authLoading) return <main className="app-shell">Loading session...</main>;
  if (!user) return <SignInPage />;
  if (workspace.isLoading) return <main className="app-shell">Preparing workspace...</main>;
  if (workspace.isError || !workspace.data) return <main className="app-shell">Workspace failed to load.</main>;

  function handleCommand(action: CommandAction) {
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
      {view === "board" ? <BoardPage workspace={workspace.data} /> : <GoalsPage workspace={workspace.data} />}
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
