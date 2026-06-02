import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, beforeEach } from "vitest";

import { ProfileEditDialog } from "@/components/profiles/ProfileEditDialog";
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

const updateMock = vi.fn().mockResolvedValue({});
const setDotfileMock = vi.fn().mockResolvedValue({});
const deleteDotfileMock = vi.fn().mockResolvedValue(true);

let dotfilesData: { profileId: string; relPath: string; content: string }[] =
  [];

vi.mock("@/hooks/useProfiles", () => ({
  useCreateProfile: () => ({
    mutateAsync: vi.fn().mockResolvedValue({}),
    isPending: false,
  }),
  useUpdateProfile: () => ({
    mutateAsync: updateMock,
    isPending: false,
  }),
  useProfileDotfiles: () => ({
    data: dotfilesData,
  }),
  useSetProfileDotfile: () => ({
    mutateAsync: setDotfileMock,
    isPending: false,
  }),
  useDeleteProfileDotfile: () => ({
    mutateAsync: deleteDotfileMock,
    isPending: false,
  }),
}));

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({
    data: { providers: {}, currentProviderId: "" },
    isLoading: false,
  }),
}));

// ---- fixtures ----

const makeSpec = () => ({
  content: { skills: [], commands: [], agents: [], mcp: [] },
  vars: {},
});

const EDIT_PROFILE: InstalledProfile = {
  id: "profile-edit-1",
  appType: "claude",
  name: "Work Profile",
  description: "For work",
  isActive: false,
  currentProviderId: undefined,
  spec: makeSpec(),
  sortIndex: 0,
  createdAt: 1700000000,
};

// ---- tests ----

describe("ProfileEditDialog", () => {
  beforeEach(() => {
    dotfilesData = [];
    updateMock.mockClear();
    setDotfileMock.mockClear();
    deleteDotfileMock.mockClear();
  });

  it("does NOT render the Dotfiles section in create mode", () => {
    render(
      <ProfileEditDialog
        open={true}
        profile={null}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    expect(
      screen.queryByText("profiles.dotfilesSection"),
    ).not.toBeInTheDocument();
  });

  it("renders the Dotfiles collapsible section in edit mode", () => {
    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByText("profiles.dotfilesSection")).toBeInTheDocument();
  });

  it("shows the two textareas after expanding the Dotfiles section", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand the collapsible
    await user.click(screen.getByText("profiles.dotfilesSection"));

    expect(
      screen.getByLabelText("profiles.settingsFragment"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("profiles.statusline")).toBeInTheDocument();
  });

  it("prefills textareas from loaded dotfiles", async () => {
    dotfilesData = [
      {
        profileId: EDIT_PROFILE.id,
        relPath: "settings.json",
        content: '{"foo": "bar"}',
      },
      {
        profileId: EDIT_PROFILE.id,
        relPath: "statusline.sh",
        content: "echo hello",
      },
    ];

    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand dotfiles section
    await user.click(screen.getByText("profiles.dotfilesSection"));

    const settingsTA = screen.getByLabelText(
      "profiles.settingsFragment",
    ) as HTMLTextAreaElement;
    const statuslineTA = screen.getByLabelText(
      "profiles.statusline",
    ) as HTMLTextAreaElement;

    expect(settingsTA.value).toBe('{"foo": "bar"}');
    expect(statuslineTA.value).toBe("echo hello");
  });

  it("calls setDotfile for non-empty content on Save", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand dotfiles section
    await user.click(screen.getByText("profiles.dotfilesSection"));

    // Type into settings.json textarea (use fireEvent to avoid user-event special-char parsing)
    const settingsTA = screen.getByLabelText("profiles.settingsFragment");
    fireEvent.change(settingsTA, {
      target: { value: '{"maxTokens":1000}' },
    });

    // Submit the form
    const saveBtn = screen.getByText("profiles.save");
    await user.click(saveBtn);

    await waitFor(() => {
      expect(setDotfileMock).toHaveBeenCalledWith(
        expect.objectContaining({
          id: EDIT_PROFILE.id,
          relPath: "settings.json",
          content: expect.stringContaining("maxTokens"),
        }),
      );
    });
  });

  it("calls deleteDotfile for cleared (empty) content on Save", async () => {
    // Pre-load a dotfile
    dotfilesData = [
      {
        profileId: EDIT_PROFILE.id,
        relPath: "statusline.sh",
        content: "echo old",
      },
    ];

    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand dotfiles section
    await user.click(screen.getByText("profiles.dotfilesSection"));

    // The statusline textarea should be prefilled; clear it
    const statuslineTA = screen.getByLabelText("profiles.statusline");
    await user.tripleClick(statuslineTA);
    await user.clear(statuslineTA);

    // Submit the form
    await user.click(screen.getByText("profiles.save"));

    await waitFor(() => {
      expect(deleteDotfileMock).toHaveBeenCalledWith(
        expect.objectContaining({
          id: EDIT_PROFILE.id,
          relPath: "statusline.sh",
        }),
      );
    });
  });
});
