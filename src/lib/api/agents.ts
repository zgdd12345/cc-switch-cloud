import { invoke } from "@tauri-apps/api/core";

/** Installed agent managed by AgentHub */
export interface InstalledAgent {
  id: string;
  name: string;
  content: string;
  description?: string;
  tags: string[];
  enabledClaude: boolean;
  installedAt: number;
}

/** Unmanaged agent found on disk */
export interface UnmanagedAgent {
  name: string;
  path: string;
}

// ========== API ==========

export const agentsApi = {
  /** Get all installed agents */
  async getInstalled(): Promise<InstalledAgent[]> {
    return await invoke("get_installed_agents");
  },

  /** Create a new agent */
  async create(
    name: string,
    content: string,
    description: string | null,
    tags: string[],
  ): Promise<InstalledAgent> {
    return await invoke("create_agent", { name, content, description, tags });
  },

  /** Update an existing agent */
  async update(
    id: string,
    content: string,
    description: string | null,
    tags: string[],
  ): Promise<InstalledAgent> {
    return await invoke("update_agent", { id, content, description, tags });
  },

  /** Delete an agent */
  async delete(id: string): Promise<boolean> {
    return await invoke("delete_agent", { id });
  },

  /** Enable or disable an agent for Claude */
  async setEnabled(id: string, enabled: boolean): Promise<boolean> {
    return await invoke("set_agent_enabled", { id, enabled });
  },

  /** Scan for unmanaged agents on disk */
  async scanUnmanaged(): Promise<UnmanagedAgent[]> {
    return await invoke("scan_unmanaged_agents");
  },

  /** Import agents from disk paths */
  async importFromDisk(paths: string[]): Promise<InstalledAgent[]> {
    return await invoke("import_agents_from_disk", { paths });
  },
};
