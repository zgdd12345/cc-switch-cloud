import { createRef } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";

import CommandsPanel, {
  type CommandsPanelHandle,
} from "@/components/commands/CommandsPanel";
import type { InstalledCommand } from "@/lib/api/commands";

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

vi.mock("@/hooks/useCommands", () => ({
  useInstalledCommands: () => ({
    data: installedCommandsData,
    isLoading: false,
  }),
  useSetCommandEnabled: () => ({
    mutateAsync: setEnabledMock,
    isPending: false,
  }),
  useDeleteCommand: () => ({
    mutateAsync: deleteMock,
    isPending: false,
  }),
  useCreateCommand: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useUpdateCommand: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
}));

// Mutable reference so tests can control the data
let installedCommandsData: InstalledCommand[] = [];

const COMMAND_A: InstalledCommand = {
  id: "cmd-a",
  name: "alpha",
  content: "Do alpha things",
  description: "Alpha description",
  tags: [],
  enabledClaude: true,
  installedAt: 1700000000,
};

const COMMAND_B: InstalledCommand = {
  id: "cmd-b",
  name: "beta",
  content: "Do beta things",
  description: undefined,
  tags: ["test"],
  enabledClaude: false,
  installedAt: 1700000001,
};

// ---- tests ----

describe("CommandsPanel", () => {
  beforeEach(() => {
    installedCommandsData = [];
    setEnabledMock.mockReset();
    deleteMock.mockReset();
  });

  it("renders empty-state when no commands are installed", () => {
    render(<CommandsPanel />);
    expect(screen.getByText("commands.empty")).toBeInTheDocument();
    expect(screen.getByText("commands.emptyDescription")).toBeInTheDocument();
  });

  it("renders a list of 2 commands showing their names", () => {
    installedCommandsData = [COMMAND_A, COMMAND_B];
    render(<CommandsPanel />);

    expect(screen.getByText("/alpha")).toBeInTheDocument();
    expect(screen.getByText("/beta")).toBeInTheDocument();
    expect(screen.getByText("Alpha description")).toBeInTheDocument();
  });

  it("clicking delete opens the ConfirmDialog for the selected command", async () => {
    installedCommandsData = [COMMAND_A, COMMAND_B];
    const user = userEvent.setup();

    render(<CommandsPanel />);

    // Hover the first row to reveal action buttons — in JSDOM opacity-0 buttons
    // are still in the DOM and clickable, so we can query by title directly.
    const deleteButtons = screen.getAllByTitle("commands.delete");
    expect(deleteButtons).toHaveLength(2);

    await user.click(deleteButtons[0]);

    await waitFor(() => {
      // ConfirmDialog renders its title
      expect(screen.getByText("commands.delete")).toBeInTheDocument();
    });
  });

  it("openCreate() via ref opens the create dialog", async () => {
    installedCommandsData = [];
    const ref = createRef<CommandsPanelHandle>();
    render(<CommandsPanel ref={ref} />);

    ref.current?.openCreate();

    await waitFor(() => {
      // CommandEditDialog shows the create title
      expect(screen.getByText("commands.create")).toBeInTheDocument();
    });
  });
});
