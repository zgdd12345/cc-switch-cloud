import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { projectsApi, type ProjectSpec } from "../projects";

describe("projectsApi project CLAUDE.md (4b-1)", () => {
  beforeEach(() => invokeMock.mockReset());

  it("ProjectSpec carries dotfiles.claudeMd", () => {
    const spec: ProjectSpec = {
      content: { skills: [], commands: [], agents: [], mcp: [] },
      vars: {},
      dotfiles: { claudeMd: "# memory" },
    };
    expect(spec.dotfiles.claudeMd).toBe("# memory");
  });

  it("save() forwards spec (incl. dotfiles.claudeMd) to project_save", async () => {
    invokeMock.mockResolvedValueOnce({ id: "proj:1" });
    const spec: ProjectSpec = {
      content: { skills: [], commands: [], agents: [], mcp: [] },
      vars: {},
      dotfiles: { claudeMd: "# project rules" },
    };
    await projectsApi.save(null, "claude", "/abs/repo", "Repo", spec, null);
    expect(invokeMock).toHaveBeenCalledWith("project_save", {
      id: null,
      app: "claude",
      enteredPath: "/abs/repo",
      name: "Repo",
      spec,
      seedFromProfileId: null,
    });
    expect(invokeMock.mock.calls[0][1].spec.dotfiles.claudeMd).toBe("# project rules");
  });
});
