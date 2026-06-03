import { invoke } from "@tauri-apps/api/core";
import type { ManifestEntry } from "./profiles";

export interface ProjectSpec {
  content: { skills: string[]; commands: string[]; agents: string[]; mcp: string[] };
  vars: Record<string, unknown>;
  // 4b-1: device-local project dotfiles. claudeMd = literal project-root CLAUDE.md.
  // 4b-2: settings = JSON fragment deep-merged into <project>/.claude/settings.json.
  dotfiles: { claudeMd: string; settings: string };
}

export interface Project {
  id: string;
  projectPath: string;
  enteredPath: string;
  appType: string;
  name?: string;
  spec: ProjectSpec;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface ProjectApplyResult {
  warnings: string[];
}

export const projectsApi = {
  async list(): Promise<Project[]> {
    return await invoke("project_list");
  },
  async get(id: string): Promise<Project | null> {
    return await invoke("project_get", { id });
  },
  async save(
    id: string | null,
    app: string,
    enteredPath: string,
    name: string | null,
    spec: ProjectSpec,
    seedFromProfileId: string | null,
  ): Promise<Project> {
    return await invoke("project_save", {
      id,
      app,
      enteredPath,
      name,
      spec,
      seedFromProfileId,
    });
  },
  async delete(id: string): Promise<boolean> {
    return await invoke("project_delete", { id });
  },
  async setEnabled(id: string, enabled: boolean): Promise<boolean> {
    return await invoke("project_set_enabled", { id, enabled });
  },
  async apply(id: string): Promise<ProjectApplyResult> {
    return await invoke("project_apply", { id });
  },
  async detach(id: string): Promise<ProjectApplyResult> {
    return await invoke("project_detach", { id });
  },
  async manifest(id: string): Promise<ManifestEntry[]> {
    return await invoke("project_manifest", { id });
  },
};
