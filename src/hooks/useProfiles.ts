import {
  useMutation,
  useQuery,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import {
  profilesApi,
  type InstalledProfile,
  type ProfileSpec,
} from "@/lib/api/profiles";

/**
 * Query all installed profiles.
 * Uses staleTime: Infinity + keepPreviousData for a cache-first experience.
 */
export function useInstalledProfiles() {
  return useQuery({
    queryKey: ["profiles", "installed"],
    queryFn: () => profilesApi.getInstalled(),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
}

/** Query the active profile for a given app */
export function useActiveProfile(app: string) {
  return useQuery({
    queryKey: ["profiles", "active", app],
    queryFn: () => profilesApi.getActive(app),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
    enabled: Boolean(app),
  });
}

/** Create a new profile */
export function useCreateProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      app,
      name,
      description,
      currentProviderId,
      spec,
    }: {
      app: string;
      name: string;
      description: string | null;
      currentProviderId: string | null;
      spec: ProfileSpec;
    }) => profilesApi.create(app, name, description, currentProviderId, spec),
    onSuccess: (created) => {
      queryClient.setQueryData<InstalledProfile[]>(
        ["profiles", "installed"],
        (old) => (old ? [...old, created] : [created]),
      );
    },
  });
}

/** Update an existing profile */
export function useUpdateProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      name,
      description,
      currentProviderId,
      spec,
    }: {
      id: string;
      name: string;
      description: string | null;
      currentProviderId: string | null;
      spec: ProfileSpec;
    }) => profilesApi.update(id, name, description, currentProviderId, spec),
    onSuccess: (updated) => {
      queryClient.setQueryData<InstalledProfile[]>(
        ["profiles", "installed"],
        (old) =>
          old ? old.map((p) => (p.id === updated.id ? updated : p)) : [updated],
      );
    },
  });
}

/** Delete a profile */
export function useDeleteProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => profilesApi.delete(id),
    onSuccess: (_result, id) => {
      queryClient.setQueryData<InstalledProfile[]>(
        ["profiles", "installed"],
        (old) => (old ? old.filter((p) => p.id !== id) : old),
      );
    },
  });
}

/** Activate a profile for an app */
export function useActivateProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ app, id }: { app: string; id: string }) =>
      profilesApi.activate(app, id),
    onSuccess: (_result, vars) => {
      // Invalidate profiles list so isActive flags refresh
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "installed"],
      });
      // Invalidate active profile cache for this app
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "active", vars.app],
      });
      // Invalidate providers so current provider badge refreshes
      void queryClient.invalidateQueries({ queryKey: ["providers"] });
      // Invalidate the four content installed lists so badges + per-item toggles refresh
      void queryClient.invalidateQueries({ queryKey: ["agents", "installed"] });
      void queryClient.invalidateQueries({
        queryKey: ["commands", "installed"],
      });
      void queryClient.invalidateQueries({ queryKey: ["skills", "installed"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp", "all"] });
    },
  });
}

/** Deactivate the active profile for an app */
export function useDeactivateProfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (app: string) => profilesApi.deactivate(app),
    onSuccess: (_result, app) => {
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "installed"],
      });
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "active", app],
      });
      void queryClient.invalidateQueries({ queryKey: ["providers"] });
      void queryClient.invalidateQueries({ queryKey: ["agents", "installed"] });
      void queryClient.invalidateQueries({
        queryKey: ["commands", "installed"],
      });
      void queryClient.invalidateQueries({ queryKey: ["skills", "installed"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp", "all"] });
    },
  });
}

/** Query all dotfiles for a given profile */
export function useProfileDotfiles(id: string | undefined) {
  return useQuery({
    queryKey: ["profiles", "dotfiles", id],
    queryFn: () => profilesApi.getDotfiles(id!),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
    enabled: Boolean(id),
  });
}

/** Upsert a dotfile for a profile */
export function useSetProfileDotfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      relPath,
      content,
    }: {
      id: string;
      relPath: string;
      content: string;
    }) => profilesApi.setDotfile(id, relPath, content),
    onSuccess: (_result, vars) => {
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "dotfiles", vars.id],
      });
    },
  });
}

/** Delete a dotfile for a profile */
export function useDeleteProfileDotfile() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, relPath }: { id: string; relPath: string }) =>
      profilesApi.deleteDotfile(id, relPath),
    onSuccess: (_result, vars) => {
      void queryClient.invalidateQueries({
        queryKey: ["profiles", "dotfiles", vars.id],
      });
    },
  });
}

// ========== re-exports ==========

export type {
  InstalledProfile,
  ProfileSpec,
  ProfileDotfile,
  ActivateProfileResult,
} from "@/lib/api/profiles";
