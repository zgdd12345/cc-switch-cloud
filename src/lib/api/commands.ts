import { invoke } from "@tauri-apps/api/core";

/** Installed slash-command managed by AgentHub */
export interface InstalledCommand {
  id: string;
  name: string;
  content: string;
  description?: string;
  tags: string[];
  enabledClaude: boolean;
  installedAt: number;
}

/** Unmanaged slash-command found on disk */
export interface UnmanagedCommand {
  name: string;
  path: string;
}

// ========== API ==========

export const commandsApi = {
  /** Get all installed commands */
  async getInstalled(): Promise<InstalledCommand[]> {
    return await invoke("get_installed_commands");
  },

  /** Create a new command */
  async create(
    name: string,
    content: string,
    description: string | null,
    tags: string[],
  ): Promise<InstalledCommand> {
    return await invoke("create_command", { name, content, description, tags });
  },

  /** Update an existing command */
  async update(
    id: string,
    content: string,
    description: string | null,
    tags: string[],
  ): Promise<InstalledCommand> {
    return await invoke("update_command", { id, content, description, tags });
  },

  /** Delete a command */
  async delete(id: string): Promise<boolean> {
    return await invoke("delete_command", { id });
  },

  /** Enable or disable a command for Claude */
  async setEnabled(id: string, enabled: boolean): Promise<boolean> {
    return await invoke("set_command_enabled", { id, enabled });
  },

  /** Scan for unmanaged commands on disk */
  async scanUnmanaged(): Promise<UnmanagedCommand[]> {
    return await invoke("scan_unmanaged_commands");
  },

  /** Import commands from disk paths */
  async importFromDisk(paths: string[]): Promise<InstalledCommand[]> {
    return await invoke("import_commands_from_disk", { paths });
  },
};
