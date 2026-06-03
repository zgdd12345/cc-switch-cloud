import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { projectsApi } from "./projects";

describe("projectsApi", () => {
  beforeEach(() => invokeMock.mockReset());

  it("save passes camelCase args including seedFromProfileId", async () => {
    invokeMock.mockResolvedValue({ id: "p1" });
    await projectsApi.save(null, "claude", "/abs/repo", "Repo", { content: { skills: [], commands: [], agents: [], mcp: [] }, vars: {} }, "local:claude:Src");
    expect(invokeMock).toHaveBeenCalledWith("project_save", {
      id: null,
      app: "claude",
      enteredPath: "/abs/repo",
      name: "Repo",
      spec: { content: { skills: [], commands: [], agents: [], mcp: [] }, vars: {} },
      seedFromProfileId: "local:claude:Src",
    });
  });

  it("apply/detach/list call the right commands", async () => {
    invokeMock.mockResolvedValue({ warnings: [] });
    await projectsApi.apply("p1");
    expect(invokeMock).toHaveBeenCalledWith("project_apply", { id: "p1" });
    await projectsApi.detach("p1");
    expect(invokeMock).toHaveBeenCalledWith("project_detach", { id: "p1" });
    invokeMock.mockResolvedValue([]);
    await projectsApi.list();
    expect(invokeMock).toHaveBeenCalledWith("project_list");
  });
});
