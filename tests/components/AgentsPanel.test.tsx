import { createRef } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";

import AgentsPanel, {
  type AgentsPanelHandle,
} from "@/components/agents/AgentsPanel";
import type { InstalledAgent } from "@/lib/api/agents";

// ---- mocks ----

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  },
}));

const setEnabledMock = vi.fn();
const deleteMock = vi.fn();

vi.mock("@/hooks/useAgents", () => ({
  useInstalledAgents: () => ({
    data: installedAgentsData,
    isLoading: false,
  }),
  useSetAgentEnabled: () => ({
    mutateAsync: setEnabledMock,
    isPending: false,
  }),
  useDeleteAgent: () => ({
    mutateAsync: deleteMock,
    isPending: false,
  }),
  useCreateAgent: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useUpdateAgent: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
}));

// Mutable reference so tests can control the data
let installedAgentsData: InstalledAgent[] = [];

const AGENT_A: InstalledAgent = {
  id: "agent-a",
  name: "alpha",
  content: "Do alpha things",
  description: "Alpha description",
  tags: [],
  enabledClaude: true,
  installedAt: 1700000000,
};

const AGENT_B: InstalledAgent = {
  id: "agent-b",
  name: "beta",
  content: "Do beta things",
  description: undefined,
  tags: ["test"],
  enabledClaude: false,
  installedAt: 1700000001,
};

// ---- tests ----

describe("AgentsPanel", () => {
  beforeEach(() => {
    installedAgentsData = [];
    setEnabledMock.mockReset();
    deleteMock.mockReset();
  });

  it("renders empty-state when no agents are installed", () => {
    render(<AgentsPanel />);
    expect(screen.getByText("agents.empty")).toBeInTheDocument();
    expect(screen.getByText("agents.emptyDescription")).toBeInTheDocument();
  });

  it("renders a list of 2 agents showing their names", () => {
    installedAgentsData = [AGENT_A, AGENT_B];
    render(<AgentsPanel />);

    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();
    expect(screen.getByText("Alpha description")).toBeInTheDocument();
  });

  it("clicking delete opens the ConfirmDialog for the selected agent", async () => {
    installedAgentsData = [AGENT_A, AGENT_B];
    const user = userEvent.setup();

    render(<AgentsPanel />);

    // Hover the first row to reveal action buttons — in JSDOM opacity-0 buttons
    // are still in the DOM and clickable, so we can query by title directly.
    const deleteButtons = screen.getAllByTitle("agents.delete");
    expect(deleteButtons).toHaveLength(2);

    await user.click(deleteButtons[0]);

    await waitFor(() => {
      // ConfirmDialog renders its title
      expect(screen.getByText("agents.delete")).toBeInTheDocument();
    });
  });

  it("openCreate() via ref opens the create dialog", async () => {
    installedAgentsData = [];
    const ref = createRef<AgentsPanelHandle>();
    render(<AgentsPanel ref={ref} />);

    ref.current?.openCreate();

    await waitFor(() => {
      // AgentEditDialog shows the create title
      expect(screen.getByText("agents.create")).toBeInTheDocument();
    });
  });
});
