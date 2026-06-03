import {
  useMutation,
  useQuery,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import { projectsApi, type Project, type ProjectSpec } from "@/lib/api/projects";

export function useProjects() {
  return useQuery({
    queryKey: ["projects", "all"],
    queryFn: () => projectsApi.list(),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
}

export function useSaveProject() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      id: string | null;
      app: string;
      enteredPath: string;
      name: string | null;
      spec: ProjectSpec;
      seedFromProfileId: string | null;
    }) =>
      projectsApi.save(
        args.id,
        args.app,
        args.enteredPath,
        args.name,
        args.spec,
        args.seedFromProfileId,
      ),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["projects", "all"] });
    },
  });
}

export function useDeleteProject() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => projectsApi.delete(id),
    onSuccess: (_r, id) => {
      qc.setQueryData<Project[]>(["projects", "all"], (old) =>
        old ? old.filter((p) => p.id !== id) : old,
      );
    },
  });
}

export function useSetProjectEnabled() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (a: { id: string; enabled: boolean }) =>
      projectsApi.setEnabled(a.id, a.enabled),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["projects", "all"] });
    },
  });
}

export function useApplyProject() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => projectsApi.apply(id),
    onSuccess: (_r, id) => {
      void qc.invalidateQueries({ queryKey: ["projects", "manifest", id] });
    },
  });
}

export function useDetachProject() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => projectsApi.detach(id),
    onSuccess: (_r, id) => {
      void qc.invalidateQueries({ queryKey: ["projects", "manifest", id] });
    },
  });
}

export function useProjectManifest(id: string | undefined) {
  return useQuery({
    queryKey: ["projects", "manifest", id],
    queryFn: () => projectsApi.manifest(id!),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
    enabled: Boolean(id),
  });
}

export type { Project, ProjectSpec } from "@/lib/api/projects";
