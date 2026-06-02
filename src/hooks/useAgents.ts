import {
  useMutation,
  useQuery,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import { agentsApi, type InstalledAgent } from "@/lib/api/agents";

/**
 * Query all installed agents.
 * Uses staleTime: Infinity + keepPreviousData for a cache-first experience.
 */
export function useInstalledAgents() {
  return useQuery({
    queryKey: ["agents", "installed"],
    queryFn: () => agentsApi.getInstalled(),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
}

/** Create a new agent */
export function useCreateAgent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      name,
      content,
      description,
      tags,
    }: {
      name: string;
      content: string;
      description: string | null;
      tags: string[];
    }) => agentsApi.create(name, content, description, tags),
    onSuccess: (created) => {
      queryClient.setQueryData<InstalledAgent[]>(
        ["agents", "installed"],
        (old) => (old ? [...old, created] : [created]),
      );
    },
  });
}

/** Update an existing agent */
export function useUpdateAgent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      content,
      description,
      tags,
    }: {
      id: string;
      content: string;
      description: string | null;
      tags: string[];
    }) => agentsApi.update(id, content, description, tags),
    onSuccess: (updated) => {
      queryClient.setQueryData<InstalledAgent[]>(
        ["agents", "installed"],
        (old) =>
          old ? old.map((a) => (a.id === updated.id ? updated : a)) : [updated],
      );
    },
  });
}

/** Enable or disable an agent for Claude */
export function useSetAgentEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      agentsApi.setEnabled(id, enabled),
    onSuccess: (_result, vars) => {
      queryClient.setQueryData<InstalledAgent[]>(
        ["agents", "installed"],
        (old) =>
          old
            ? old.map((a) =>
                a.id === vars.id ? { ...a, enabledClaude: vars.enabled } : a,
              )
            : old,
      );
    },
  });
}

/** Delete an agent */
export function useDeleteAgent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => agentsApi.delete(id),
    onSuccess: (_result, id) => {
      queryClient.setQueryData<InstalledAgent[]>(
        ["agents", "installed"],
        (old) => (old ? old.filter((a) => a.id !== id) : old),
      );
    },
  });
}

// ========== re-exports ==========

export type { InstalledAgent, UnmanagedAgent } from "@/lib/api/agents";
