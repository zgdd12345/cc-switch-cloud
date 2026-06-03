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

const createMockFn = vi.fn().mockResolvedValue({});
const updateMock = vi.fn().mockResolvedValue({});
const setDotfileMock = vi.fn().mockResolvedValue({});
const deleteDotfileMock = vi.fn().mockResolvedValue(true);

let dotfilesData: { profileId: string; relPath: string; content: string }[] =
  [];

vi.mock("@/hooks/useProfiles", () => ({
  useCreateProfile: () => ({
    mutateAsync: createMockFn,
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
    createMockFn.mockClear();
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

  it("shows the CLAUDE.md textarea after expanding the Dotfiles section", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    await user.click(screen.getByText("profiles.dotfilesSection"));

    expect(screen.getByLabelText("profiles.claudeMd")).toBeInTheDocument();
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

  it("prefills CLAUDE.md textarea from loaded dotfiles", async () => {
    dotfilesData = [
      {
        profileId: EDIT_PROFILE.id,
        relPath: "CLAUDE.md",
        content: "# My Project Instructions\n\nDo not use emojis.",
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

    await user.click(screen.getByText("profiles.dotfilesSection"));

    const claudeMdTA = screen.getByLabelText(
      "profiles.claudeMd",
    ) as HTMLTextAreaElement;

    expect(claudeMdTA.value).toBe(
      "# My Project Instructions\n\nDo not use emojis.",
    );
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

  it("calls setDotfile with rel_path CLAUDE.md for non-empty content on Save", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={EDIT_PROFILE}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    await user.click(screen.getByText("profiles.dotfilesSection"));

    const claudeMdTA = screen.getByLabelText("profiles.claudeMd");
    fireEvent.change(claudeMdTA, {
      target: { value: "# Profile instructions\n\nBe concise." },
    });

    await user.click(screen.getByText("profiles.save"));

    await waitFor(() => {
      expect(setDotfileMock).toHaveBeenCalledWith(
        expect.objectContaining({
          id: EDIT_PROFILE.id,
          relPath: "CLAUDE.md",
          content: expect.stringContaining("Profile instructions"),
        }),
      );
    });
  });

  it("calls deleteDotfile with rel_path CLAUDE.md for cleared content on Save", async () => {
    dotfilesData = [
      {
        profileId: EDIT_PROFILE.id,
        relPath: "CLAUDE.md",
        content: "# Old instructions",
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

    await user.click(screen.getByText("profiles.dotfilesSection"));

    const claudeMdTA = screen.getByLabelText("profiles.claudeMd");
    await user.tripleClick(claudeMdTA);
    await user.clear(claudeMdTA);

    await user.click(screen.getByText("profiles.save"));

    await waitFor(() => {
      expect(deleteDotfileMock).toHaveBeenCalledWith(
        expect.objectContaining({
          id: EDIT_PROFILE.id,
          relPath: "CLAUDE.md",
        }),
      );
    });
  });

  // ---- Variables editor tests ----

  it("renders the Variables collapsible section in both create and edit mode", () => {
    render(
      <ProfileEditDialog
        open={true}
        profile={null}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText("profiles.variables")).toBeInTheDocument();
  });

  it("add-row button adds a key/value input pair", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={null}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand the Variables section
    await user.click(screen.getByText("profiles.variables"));

    // Initially no key inputs
    expect(screen.queryAllByPlaceholderText("profiles.varKey")).toHaveLength(0);

    // Click Add Variable
    await user.click(screen.getByText("profiles.addVar"));

    // Now one pair should appear
    expect(screen.getAllByPlaceholderText("profiles.varKey")).toHaveLength(1);
    expect(screen.getAllByPlaceholderText("profiles.varValue")).toHaveLength(1);
  });

  it("Save includes vars in the spec passed to create mutation", async () => {
    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={null}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Fill required name field
    const nameInput = screen.getByPlaceholderText("profiles.namePlaceholder");
    await user.type(nameInput, "My Profile");

    // Expand Variables and add a row
    await user.click(screen.getByText("profiles.variables"));
    await user.click(screen.getByText("profiles.addVar"));

    const keyInput = screen.getByPlaceholderText("profiles.varKey");
    const valInput = screen.getByPlaceholderText("profiles.varValue");

    fireEvent.change(keyInput, { target: { value: "MY_VAR" } });
    fireEvent.change(valInput, { target: { value: "hello" } });

    // Submit
    await user.click(screen.getByText("profiles.save"));

    await waitFor(() => {
      expect(createMockFn).toHaveBeenCalledWith(
        expect.objectContaining({
          spec: expect.objectContaining({
            vars: expect.objectContaining({ MY_VAR: "hello" }),
          }),
        }),
      );
    });
  });

  it("Save includes vars in the spec passed to update mutation", async () => {
    const profileWithVars: InstalledProfile = {
      ...EDIT_PROFILE,
      spec: {
        content: { skills: [], commands: [], agents: [], mcp: [] },
        vars: { EXISTING: "old" },
      },
    };

    const user = userEvent.setup();

    render(
      <ProfileEditDialog
        open={true}
        profile={profileWithVars}
        currentApp="claude"
        onClose={vi.fn()}
      />,
    );

    // Expand Variables — EXISTING row should be pre-filled
    await user.click(screen.getByText("profiles.variables"));

    const valInputs = screen.getAllByPlaceholderText("profiles.varValue");
    expect(valInputs).toHaveLength(1);
    expect((valInputs[0] as HTMLInputElement).value).toBe("old");

    // Update the value
    fireEvent.change(valInputs[0], { target: { value: "new" } });

    // Submit
    await user.click(screen.getByText("profiles.save"));

    await waitFor(() => {
      expect(updateMock).toHaveBeenCalledWith(
        expect.objectContaining({
          spec: expect.objectContaining({
            vars: expect.objectContaining({ EXISTING: "new" }),
          }),
        }),
      );
    });
  });
});
