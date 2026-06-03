import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";

import UnifiedSkillsPanel from "@/components/skills/UnifiedSkillsPanel";
import type { InstalledSkill } from "@/lib/api/skills";

// ---- mocks ----

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  },
}));

const updateSkillTagsMock = vi.fn();

const SKILL_A: InstalledSkill = {
  id: "skill-a",
  name: "my-skill",
  description: "A test skill",
  directory: "my-skill",
  apps: {
    claude: true,
    codex: false,
    gemini: false,
    opencode: false,
    openclaw: false,
    hermes: false,
  },
  installedAt: 1700000000,
  updatedAt: 1700000000,
  tags: ["core", "backend"],
};

vi.mock("@/hooks/useSkills", () => ({
  useInstalledSkills: () => ({
    data: [SKILL_A],
    isLoading: false,
  }),
  useSkillBackups: () => ({
    data: [],
    refetch: vi.fn(),
    isFetching: false,
  }),
  useDeleteSkillBackup: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useToggleSkillApp: () => ({
    mutateAsync: vi.fn(),
  }),
  useRestoreSkillBackup: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useUninstallSkill: () => ({
    mutateAsync: vi.fn(),
  }),
  useScanUnmanagedSkills: () => ({
    data: [],
    refetch: vi.fn(),
  }),
  useImportSkillsFromApps: () => ({
    mutateAsync: vi.fn(),
  }),
  useInstallSkillsFromZip: () => ({
    mutateAsync: vi.fn(),
  }),
  useCheckSkillUpdates: () => ({
    data: [],
    refetch: vi.fn(),
    isFetching: false,
  }),
  useUpdateSkill: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useUpdateSkillTags: () => ({
    mutateAsync: updateSkillTagsMock,
    isPending: false,
  }),
}));

describe("SkillTagsInput in UnifiedSkillsPanel", () => {
  beforeEach(() => {
    updateSkillTagsMock.mockReset();
    updateSkillTagsMock.mockResolvedValue({ ...SKILL_A });
  });

  it("renders the tags input prefilled from skill.tags", () => {
    render(
      <UnifiedSkillsPanel onOpenDiscovery={() => {}} currentApp="claude" />,
    );

    const input = screen.getByDisplayValue("core, backend");
    expect(input).toBeInTheDocument();
  });

  it("calls updateSkillTags with parsed array on blur", async () => {
    const user = userEvent.setup();

    render(
      <UnifiedSkillsPanel onOpenDiscovery={() => {}} currentApp="claude" />,
    );

    const input = screen.getByDisplayValue("core, backend");

    // Clear and type new tags
    await user.clear(input);
    await user.type(input, "frontend, @ui");
    await user.tab(); // triggers onBlur

    await waitFor(() => {
      expect(updateSkillTagsMock).toHaveBeenCalledWith({
        id: "skill-a",
        tags: ["frontend", "@ui"],
      });
    });
  });

  it("calls updateSkillTags on Enter keypress", async () => {
    const user = userEvent.setup();

    render(
      <UnifiedSkillsPanel onOpenDiscovery={() => {}} currentApp="claude" />,
    );

    const input = screen.getByDisplayValue("core, backend");

    await user.clear(input);
    await user.type(input, "@core, tools");
    await user.keyboard("{Enter}");

    await waitFor(() => {
      expect(updateSkillTagsMock).toHaveBeenCalledWith({
        id: "skill-a",
        tags: ["@core", "tools"],
      });
    });
  });
});
