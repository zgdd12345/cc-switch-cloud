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
import { useCreateProfile, useUpdateProfile } from "@/hooks/useProfiles";
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

  const createMutation = useCreateProfile();
  const updateMutation = useUpdateProfile();
  const isPending = createMutation.isPending || updateMutation.isPending;

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
      } else {
        setName("");
        setDescription("");
        setCurrentProviderId("");
        setSkills("");
        setCommands("");
        setAgents("");
        setMcp("");
      }
    }
  }, [open, profile]);

  const buildSpec = (): ProfileSpec => ({
    content: {
      skills: stringToList(skills),
      commands: stringToList(commands),
      agents: stringToList(agents),
      mcp: stringToList(mcp),
    },
    vars: profile?.spec.vars ?? {},
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
