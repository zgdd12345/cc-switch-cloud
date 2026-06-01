import {
  useMutation,
  useQuery,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import { commandsApi, type InstalledCommand } from "@/lib/api/commands";

/**
 * Query all installed commands.
 * Uses staleTime: Infinity + keepPreviousData for a cache-first experience.
 */
export function useInstalledCommands() {
  return useQuery({
    queryKey: ["commands", "installed"],
    queryFn: () => commandsApi.getInstalled(),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
}

/** Create a new command */
export function useCreateCommand() {
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
    }) => commandsApi.create(name, content, description, tags),
    onSuccess: (created) => {
      queryClient.setQueryData<InstalledCommand[]>(
        ["commands", "installed"],
        (old) => (old ? [...old, created] : [created]),
      );
    },
  });
}

/** Update an existing command */
export function useUpdateCommand() {
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
    }) => commandsApi.update(id, content, description, tags),
    onSuccess: (updated) => {
      queryClient.setQueryData<InstalledCommand[]>(
        ["commands", "installed"],
        (old) =>
          old ? old.map((c) => (c.id === updated.id ? updated : c)) : [updated],
      );
    },
  });
}

/** Enable or disable a command for Claude */
export function useSetCommandEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      commandsApi.setEnabled(id, enabled),
    onSuccess: (_result, vars) => {
      queryClient.setQueryData<InstalledCommand[]>(
        ["commands", "installed"],
        (old) =>
          old
            ? old.map((c) =>
                c.id === vars.id ? { ...c, enabledClaude: vars.enabled } : c,
              )
            : old,
      );
    },
  });
}

/** Delete a command */
export function useDeleteCommand() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => commandsApi.delete(id),
    onSuccess: (_result, id) => {
      queryClient.setQueryData<InstalledCommand[]>(
        ["commands", "installed"],
        (old) => (old ? old.filter((c) => c.id !== id) : old),
      );
    },
  });
}

// ========== re-exports ==========

export type { InstalledCommand, UnmanagedCommand } from "@/lib/api/commands";
