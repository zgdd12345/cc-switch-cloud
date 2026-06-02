import { invoke } from "@tauri-apps/api/core";

export interface ProfileSpec {
  content: {
    skills: string[];
    commands: string[];
    agents: string[];
    mcp: string[];
  };
  vars: Record<string, unknown>;
}

/** Installed profile managed by AgentHub */
export interface InstalledProfile {
  id: string;
  appType: string;
  name: string;
  description?: string;
  isActive: boolean;
  currentProviderId?: string;
  spec: ProfileSpec;
  sortIndex: number;
  createdAt: number;
}

export interface ActivateProfileResult {
  warnings: string[];
}

// ========== API ==========

export const profilesApi = {
  /** Get all installed profiles */
  async getInstalled(): Promise<InstalledProfile[]> {
    return await invoke("get_profiles");
  },

  /** Get profiles for a specific app */
  async getForApp(app: string): Promise<InstalledProfile[]> {
    return await invoke("get_profiles_for_app", { app });
  },

  /** Create a new profile */
  async create(
    app: string,
    name: string,
    description: string | null,
    currentProviderId: string | null,
    spec: ProfileSpec,
  ): Promise<InstalledProfile> {
    return await invoke("create_profile", {
      app,
      name,
      description,
      currentProviderId,
      spec,
    });
  },

  /** Update an existing profile */
  async update(
    id: string,
    name: string,
    description: string | null,
    currentProviderId: string | null,
    spec: ProfileSpec,
  ): Promise<InstalledProfile> {
    return await invoke("update_profile", {
      id,
      name,
      description,
      currentProviderId,
      spec,
    });
  },

  /** Delete a profile */
  async delete(id: string): Promise<boolean> {
    return await invoke("delete_profile", { id });
  },

  /** Activate a profile for an app */
  async activate(app: string, id: string): Promise<ActivateProfileResult> {
    return await invoke("activate_profile", { app, id });
  },

  /** Deactivate the active profile for an app */
  async deactivate(app: string): Promise<void> {
    return await invoke("deactivate_profile", { app });
  },

  /** Get the currently active profile for an app */
  async getActive(app: string): Promise<InstalledProfile | null> {
    return await invoke("get_active_profile", { app });
  },
};
