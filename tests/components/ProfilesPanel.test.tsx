import { createRef } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";

import ProfilesPanel, {
  type ProfilesPanelHandle,
} from "@/components/profiles/ProfilesPanel";
import type { InstalledProfile } from "@/lib/api/profiles";

// ---- mocks ----

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

const deleteMock = vi.fn();
const activateMock = vi.fn();

vi.mock("@/hooks/useProfiles", () => ({
  useInstalledProfiles: () => ({
    data: installedProfilesData,
    isLoading: false,
  }),
  useDeleteProfile: () => ({
    mutateAsync: deleteMock,
    isPending: false,
  }),
  useActivateProfile: () => ({
    mutateAsync: activateMock,
    isPending: false,
  }),
  useCreateProfile: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useUpdateProfile: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useProfileDotfiles: () => ({
    data: [],
  }),
  useSetProfileDotfile: () => ({
    mutateAsync: vi.fn().mockResolvedValue({}),
    isPending: false,
  }),
  useDeleteProfileDotfile: () => ({
    mutateAsync: vi.fn().mockResolvedValue(true),
    isPending: false,
  }),
  useProfileManifest: () => ({
    data: [],
  }),
}));

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({
    data: { providers: {}, currentProviderId: "" },
    isLoading: false,
  }),
}));

// Mutable reference so tests can control the data
let installedProfilesData: InstalledProfile[] = [];

const makeSpec = () => ({
  content: { skills: [], commands: [], agents: [], mcp: [] },
  vars: {},
});

const PROFILE_A: InstalledProfile = {
  id: "profile-a",
  appType: "claude",
  name: "Work Profile",
  description: "Profile for work tasks",
  isActive: true,
  currentProviderId: "provider-1",
  spec: makeSpec(),
  sortIndex: 0,
  createdAt: 1700000000,
};

const PROFILE_B: InstalledProfile = {
  id: "profile-b",
  appType: "claude",
  name: "Personal Profile",
  description: undefined,
  isActive: false,
  currentProviderId: undefined,
  spec: makeSpec(),
  sortIndex: 1,
  createdAt: 1700000001,
};

// ---- tests ----

describe("ProfilesPanel", () => {
  beforeEach(() => {
    installedProfilesData = [];
    deleteMock.mockReset();
    activateMock.mockReset();
  });

  it("renders empty-state when no profiles are installed", () => {
    render(<ProfilesPanel />);
    expect(screen.getByText("profiles.empty")).toBeInTheDocument();
    expect(screen.getByText("profiles.emptyDescription")).toBeInTheDocument();
  });

  it("renders a list of 2 profiles showing their names", () => {
    installedProfilesData = [PROFILE_A, PROFILE_B];
    render(<ProfilesPanel />);

    expect(screen.getByText("Work Profile")).toBeInTheDocument();
    expect(screen.getByText("Personal Profile")).toBeInTheDocument();
    expect(screen.getByText("Profile for work tasks")).toBeInTheDocument();
  });

  it("renders the Active badge on the isActive profile", () => {
    installedProfilesData = [PROFILE_A, PROFILE_B];
    render(<ProfilesPanel />);

    // PROFILE_A has isActive: true — it should have an Active badge
    const badges = screen.getAllByTestId("active-badge");
    expect(badges).toHaveLength(1);
    expect(badges[0]).toHaveTextContent("profiles.active");
  });

  it("clicking delete opens the ConfirmDialog for the selected profile", async () => {
    installedProfilesData = [PROFILE_A, PROFILE_B];
    const user = userEvent.setup();

    render(<ProfilesPanel />);

    // Buttons are in the DOM even with opacity-0 in JSDOM
    const deleteButtons = screen.getAllByTitle("profiles.delete");
    expect(deleteButtons).toHaveLength(2);

    await user.click(deleteButtons[0]);

    await waitFor(() => {
      expect(screen.getByText("profiles.delete")).toBeInTheDocument();
    });
  });

  it("openCreate() via ref opens the create dialog", async () => {
    installedProfilesData = [];
    const ref = createRef<ProfilesPanelHandle>();
    render(<ProfilesPanel ref={ref} />);

    ref.current?.openCreate();

    await waitFor(() => {
      // ProfileEditDialog shows the create title
      expect(screen.getByText("profiles.create")).toBeInTheDocument();
    });
  });
});
