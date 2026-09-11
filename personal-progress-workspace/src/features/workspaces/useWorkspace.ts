import { useQuery } from "@tanstack/react-query";
import { useAuth } from "../auth/AuthProvider";
import { getOrCreatePersonalWorkspace } from "./workspaceApi";

export function useWorkspace() {
  const { user } = useAuth();

  return useQuery({
    queryKey: ["workspace", user?.id],
    queryFn: () => {
      if (!user) throw new Error("User is required to load workspace");
      return getOrCreatePersonalWorkspace(user);
    },
    enabled: Boolean(user),
  });
}
