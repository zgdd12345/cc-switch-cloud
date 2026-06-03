import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import {
  useCreateProfile,
  useUpdateProfile,
  useProfileDotfiles,
  useSetProfileDotfile,
  useDeleteProfileDotfile,
  useProfileManifest,
} from "@/hooks/useProfiles";
import type { InstalledProfile, ProfileSpec } from "@/lib/api/profiles";
import { useProvidersQuery } from "@/lib/query/queries";
import type { AppId } from "@/lib/api";
import { toast } from "sonner";

interface ProfileEditDialogProps {
  open: boolean;
  /** When provided the dialog is in edit mode; otherwise create mode */
  profile?: InstalledProfile | null;
  currentApp?: string;
  onClose: () => void;
}

const listToString = (arr: string[]) => arr.join(", ");

const stringToList = (s: string): string[] =>
  s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);

interface VarRow {
  key: string;
  value: string;
}

const varsToRows = (vars: Record<string, unknown>): VarRow[] =>
  Object.entries(vars).map(([key, value]) => ({
    key,
    value: String(value ?? ""),
  }));

const rowsToVars = (rows: VarRow[]): Record<string, string> => {
  const result: Record<string, string> = {};
  for (const { key, value } of rows) {
    if (key.trim()) {
      result[key.trim()] = value;
    }
  }
  return result;
};

export const ProfileEditDialog: React.FC<ProfileEditDialogProps> = ({
  open,
  profile,
  currentApp,
  onClose,
}) => {
  const { t } = useTranslation();
  const isEdit = Boolean(profile);

  const appId = (profile?.appType ?? currentApp ?? "claude") as AppId;
  const { data: providersData } = useProvidersQuery(appId);
  const providers = providersData ? Object.values(providersData.providers) : [];

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [currentProviderId, setCurrentProviderId] = useState("");

  // Spec content lists as comma-separated strings
  const [skills, setSkills] = useState("");
  const [commands, setCommands] = useState("");
  const [agents, setAgents] = useState("");
  const [mcp, setMcp] = useState("");

  // Dotfile textarea state (edit mode only)
  const [settingsContent, setSettingsContent] = useState("");
  const [statuslineContent, setStatuslineContent] = useState("");
  const [claudeMdContent, setClaudeMdContent] = useState("");
  const [dotfilesOpen, setDotfilesOpen] = useState(false);

  // Variables editor state
  const [varRows, setVarRows] = useState<VarRow[]>([]);
  const [varsOpen, setVarsOpen] = useState(false);

  const createMutation = useCreateProfile();
  const updateMutation = useUpdateProfile();
  const setDotfileMutation = useSetProfileDotfile();
  const deleteDotfileMutation = useDeleteProfileDotfile();
  const isPending =
    createMutation.isPending ||
    updateMutation.isPending ||
    setDotfileMutation.isPending ||
    deleteDotfileMutation.isPending;

  // Load existing dotfiles when editing
  const { data: dotfiles } = useProfileDotfiles(
    isEdit && open ? profile?.id : undefined,
  );

  // Load applied-files manifest when editing
  const { data: manifestEntries } = useProfileManifest(
    isEdit && open ? profile?.id : undefined,
    isEdit && open ? appId : undefined,
  );

  // Sync form when dialog opens
  useEffect(() => {
    if (open) {
      if (profile) {
        setName(profile.name);
        setDescription(profile.description ?? "");
        setCurrentProviderId(profile.currentProviderId ?? "");
        setSkills(listToString(profile.spec.content.skills));
        setCommands(listToString(profile.spec.content.commands));
        setAgents(listToString(profile.spec.content.agents));
        setMcp(listToString(profile.spec.content.mcp));
        setVarRows(varsToRows(profile.spec.vars));
      } else {
        setName("");
        setDescription("");
        setCurrentProviderId("");
        setSkills("");
        setCommands("");
        setAgents("");
        setMcp("");
        setSettingsContent("");
        setStatuslineContent("");
        setClaudeMdContent("");
        setDotfilesOpen(false);
        setVarRows([]);
        setVarsOpen(false);
      }
    }
  }, [open, profile]);

  // Prefill dotfile textareas when dotfiles load
  useEffect(() => {
    if (dotfiles) {
      const settings = dotfiles.find((d) => d.relPath === "settings.json");
      const statusline = dotfiles.find((d) => d.relPath === "statusline.sh");
      const claudeMd = dotfiles.find((d) => d.relPath === "CLAUDE.md");
      setSettingsContent(settings?.content ?? "");
      setStatuslineContent(statusline?.content ?? "");
      setClaudeMdContent(claudeMd?.content ?? "");
    }
  }, [dotfiles]);

  const buildSpec = (): ProfileSpec => ({
    content: {
      skills: stringToList(skills),
      commands: stringToList(commands),
      agents: stringToList(agents),
      mcp: stringToList(mcp),
    },
    vars: rowsToVars(varRows),
  });

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    const spec = buildSpec();
    const descriptionValue = description.trim() || null;
    const providerIdValue = currentProviderId.trim() || null;

    try {
      if (isEdit && profile) {
        await updateMutation.mutateAsync({
          id: profile.id,
          name,
          description: descriptionValue,
          currentProviderId: providerIdValue,
          spec,
        });

        // Handle dotfile saves/deletes
        const dotfileOps: Promise<unknown>[] = [];
        for (const { relPath, value } of [
          { relPath: "settings.json", value: settingsContent },
          { relPath: "statusline.sh", value: statuslineContent },
          { relPath: "CLAUDE.md", value: claudeMdContent },
        ]) {
          if (value.trim()) {
            dotfileOps.push(
              setDotfileMutation.mutateAsync({
                id: profile.id,
                relPath,
                content: value,
              }),
            );
          } else {
            dotfileOps.push(
              deleteDotfileMutation.mutateAsync({
                id: profile.id,
                relPath,
              }),
            );
          }
        }
        await Promise.all(dotfileOps);

        toast.success(t("profiles.updateSuccess", { name }));
      } else {
        await createMutation.mutateAsync({
          app: currentApp ?? appId,
          name,
          description: descriptionValue,
          currentProviderId: providerIdValue,
          spec,
        });
        toast.success(t("profiles.createSuccess", { name }));
      }
      onClose();
    } catch (error) {
      toast.error(t("common.error"), { description: String(error) });
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-lg" zIndex="alert">
        <DialogHeader>
          <DialogTitle>
            {isEdit ? t("profiles.edit") : t("profiles.create")}
          </DialogTitle>
        </DialogHeader>

        <form onSubmit={(e) => void handleSubmit(e)} className="space-y-4 py-2">
          {/* Name */}
          <div className="space-y-1">
            <label className="text-sm font-medium">{t("profiles.name")}</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("profiles.namePlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
              required
              autoFocus
            />
          </div>

          {/* Description */}
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("profiles.description")}
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("profiles.descriptionPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          {/* Provider picker */}
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("profiles.provider")}
            </label>
            <select
              value={currentProviderId}
              onChange={(e) => setCurrentProviderId(e.target.value)}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="">{t("profiles.providerNone")}</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>

          {/* Spec content: skills */}
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("profiles.skills")}
            </label>
            <input
              type="text"
              value={skills}
              onChange={(e) => setSkills(e.target.value)}
              placeholder={t("profiles.skillsPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          {/* Spec content: commands */}
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("profiles.commands")}
            </label>
            <input
              type="text"
              value={commands}
              onChange={(e) => setCommands(e.target.value)}
              placeholder={t("profiles.commandsPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          {/* Spec content: agents */}
          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("profiles.agents")}
            </label>
            <input
              type="text"
              value={agents}
              onChange={(e) => setAgents(e.target.value)}
              placeholder={t("profiles.agentsPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          {/* Spec content: mcp servers */}
          <div className="space-y-1">
            <label className="text-sm font-medium">{t("profiles.mcp")}</label>
            <input
              type="text"
              value={mcp}
              onChange={(e) => setMcp(e.target.value)}
              placeholder={t("profiles.mcpPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          {/* Variables section — both create and edit mode */}
          <div className="space-y-2 rounded-md border border-input p-3">
            <button
              type="button"
              className="flex w-full items-center justify-between text-sm font-medium"
              onClick={() => setVarsOpen((v) => !v)}
            >
              <span>{t("profiles.variables")}</span>
              <span className="text-muted-foreground text-xs">
                {varsOpen ? "▲" : "▼"}
              </span>
            </button>

            {varsOpen && (
              <div className="space-y-2 pt-1">
                <p className="text-muted-foreground text-xs">
                  {t("profiles.variablesHint")}
                </p>
                {varRows.map((row, idx) => (
                  <div key={idx} className="flex items-center gap-2">
                    <input
                      type="text"
                      value={row.key}
                      onChange={(e) => {
                        const next = [...varRows];
                        next[idx] = { ...next[idx], key: e.target.value };
                        setVarRows(next);
                      }}
                      placeholder={t("profiles.varKey")}
                      aria-label={t("profiles.varKey")}
                      className="w-1/3 rounded-md border border-input bg-background px-2 py-1 font-mono text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                    <input
                      type="text"
                      value={row.value}
                      onChange={(e) => {
                        const next = [...varRows];
                        next[idx] = { ...next[idx], value: e.target.value };
                        setVarRows(next);
                      }}
                      placeholder={t("profiles.varValue")}
                      aria-label={t("profiles.varValue")}
                      className="flex-1 rounded-md border border-input bg-background px-2 py-1 font-mono text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                    <button
                      type="button"
                      onClick={() =>
                        setVarRows(varRows.filter((_, i) => i !== idx))
                      }
                      className="text-muted-foreground hover:text-destructive text-xs"
                      aria-label="delete-var-row"
                    >
                      ✕
                    </button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setVarRows([...varRows, { key: "", value: "" }])
                  }
                >
                  {t("profiles.addVar")}
                </Button>
              </div>
            )}
          </div>

          {/* Dotfiles section — edit mode only */}
          {isEdit && (
            <div className="space-y-2 rounded-md border border-input p-3">
              <button
                type="button"
                className="flex w-full items-center justify-between text-sm font-medium"
                onClick={() => setDotfilesOpen((v) => !v)}
              >
                <span>{t("profiles.dotfilesSection")}</span>
                <span className="text-muted-foreground text-xs">
                  {dotfilesOpen ? "▲" : "▼"}
                </span>
              </button>

              {dotfilesOpen && (
                <div className="space-y-3 pt-1">
                  {/* settings.json */}
                  <div className="space-y-1">
                    <label className="text-sm font-medium">
                      {t("profiles.settingsFragment")}
                    </label>
                    <p className="text-muted-foreground text-xs">
                      {t("profiles.settingsFragmentHint")}
                    </p>
                    <textarea
                      value={settingsContent}
                      onChange={(e) => setSettingsContent(e.target.value)}
                      rows={5}
                      spellCheck={false}
                      aria-label={t("profiles.settingsFragment")}
                      className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                  </div>

                  {/* statusline.sh */}
                  <div className="space-y-1">
                    <label className="text-sm font-medium">
                      {t("profiles.statusline")}
                    </label>
                    <p className="text-muted-foreground text-xs">
                      {t("profiles.statuslineHint")}
                    </p>
                    <textarea
                      value={statuslineContent}
                      onChange={(e) => setStatuslineContent(e.target.value)}
                      rows={4}
                      spellCheck={false}
                      aria-label={t("profiles.statusline")}
                      className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                  </div>

                  {/* CLAUDE.md */}
                  <div className="space-y-1">
                    <label className="text-sm font-medium">
                      {t("profiles.claudeMd")}
                    </label>
                    <p className="text-muted-foreground text-xs">
                      {t("profiles.claudeMdHint")}
                    </p>
                    <textarea
                      value={claudeMdContent}
                      onChange={(e) => setClaudeMdContent(e.target.value)}
                      rows={6}
                      spellCheck={false}
                      aria-label={t("profiles.claudeMd")}
                      className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                    />
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Applied files section — edit mode only */}
          {isEdit && (
            <div className="space-y-1 rounded-md border border-input p-3">
              <p className="text-sm font-medium">
                {t("profiles.appliedFiles")}
              </p>
              {manifestEntries && manifestEntries.length > 0 ? (
                <ul className="space-y-1 pt-1">
                  {manifestEntries.map((entry) => (
                    <li
                      key={entry.id}
                      className="text-muted-foreground truncate font-mono text-xs"
                      title={entry.targetPath}
                    >
                      {entry.targetPath}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-muted-foreground text-xs">—</p>
              )}
            </div>
          )}

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={onClose}
              disabled={isPending}
            >
              {t("profiles.cancel")}
            </Button>
            <Button type="submit" disabled={isPending}>
              {t("profiles.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default ProfileEditDialog;
