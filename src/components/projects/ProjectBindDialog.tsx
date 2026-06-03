import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { toast } from "sonner";
import { useSaveProject, useProjectManifest } from "@/hooks/useProjects";
import { useInstalledProfiles } from "@/hooks/useProfiles";
import type { Project, ProjectSpec } from "@/lib/api/projects";

interface Props {
  open: boolean;
  project: Project | null;
  currentApp: string;
  onClose: () => void;
}

const splitList = (s: string): string[] =>
  s.split(",").map((x) => x.trim()).filter(Boolean);

export const ProjectBindDialog: React.FC<Props> = ({ open, project, currentApp, onClose }) => {
  const { t } = useTranslation();
  const save = useSaveProject();
  const { data: profiles } = useInstalledProfiles();
  const { data: manifest } = useProjectManifest(project?.id);

  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [skills, setSkills] = useState("");
  const [commands, setCommands] = useState("");
  const [agents, setAgents] = useState("");
  const [seedId, setSeedId] = useState<string>("");

  useEffect(() => {
    if (!open) return;
    setPath(project?.enteredPath ?? "");
    setName(project?.name ?? "");
    setSkills((project?.spec.content.skills ?? []).join(", "));
    setCommands((project?.spec.content.commands ?? []).join(", "));
    setAgents((project?.spec.content.agents ?? []).join(", "));
    setSeedId("");
  }, [open, project]);

  if (!open) return null;

  const pickDir = async () => {
    const sel = await openDialog({ directory: true, multiple: false });
    if (typeof sel === "string") setPath(sel);
  };

  const onSave = async () => {
    const spec: ProjectSpec = {
      content: {
        skills: splitList(skills),
        commands: splitList(commands),
        agents: splitList(agents),
        mcp: [],
      },
      vars: {},
    };
    try {
      await save.mutateAsync({
        id: project?.id ?? null,
        app: currentApp,
        enteredPath: path,
        name: name || null,
        spec,
        seedFromProfileId: seedId || null,
      });
      toast.success(t(project ? "projects.updateSuccess" : "projects.createSuccess", { name: name || path }));
      onClose();
    } catch (e) {
      toast.error(t("common.error"), { description: String(e) });
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={onClose}>
      <div className="bg-background rounded-xl p-6 w-[560px] max-h-[80vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
        <h2 className="text-lg font-semibold mb-4">{t(project ? "projects.edit" : "projects.create")}</h2>

        <label className="text-sm font-medium">{t("projects.directory")}</label>
        <div className="flex gap-2 mt-1 mb-3">
          <Input value={path} onChange={(e) => setPath(e.target.value)} placeholder={t("projects.directoryPlaceholder")} />
          <Button variant="outline" onClick={() => void pickDir()}>{t("projects.browse")}</Button>
        </div>

        <label className="text-sm font-medium">{t("projects.name")}</label>
        <Input className="mt-1 mb-3" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("projects.namePlaceholder")} />

        {!project && (
          <>
            <label className="text-sm font-medium">{t("projects.seedFromProfile")}</label>
            <select className="mt-1 mb-1 w-full rounded-md border border-border-default bg-background p-2 text-sm" value={seedId} onChange={(e) => setSeedId(e.target.value)}>
              <option value="">{t("projects.seedNone")}</option>
              {(profiles ?? []).filter((p) => p.appType === currentApp).map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
            <p className="text-xs text-muted-foreground mb-3">{t("projects.seedHint")}</p>
          </>
        )}

        <label className="text-sm font-medium">{t("projects.skills")}</label>
        <Input className="mt-1 mb-3" value={skills} onChange={(e) => setSkills(e.target.value)} placeholder={t("projects.skillsPlaceholder")} />
        <label className="text-sm font-medium">{t("projects.commands")}</label>
        <Input className="mt-1 mb-3" value={commands} onChange={(e) => setCommands(e.target.value)} placeholder={t("projects.commandsPlaceholder")} />
        <label className="text-sm font-medium">{t("projects.agents")}</label>
        <Input className="mt-1 mb-3" value={agents} onChange={(e) => setAgents(e.target.value)} placeholder={t("projects.agentsPlaceholder")} />

        {project && (manifest?.length ?? 0) > 0 && (
          <div className="mb-3">
            <label className="text-sm font-medium">{t("projects.appliedFiles")}</label>
            <ul className="mt-1 text-xs text-muted-foreground max-h-32 overflow-y-auto">
              {manifest!.map((m) => (
                <li key={m.id} className="truncate" title={m.targetPath}>{m.kind}: {m.targetPath}</li>
              ))}
            </ul>
          </div>
        )}

        <div className="flex justify-end gap-2 mt-4">
          <Button variant="ghost" onClick={onClose}>{t("projects.cancel")}</Button>
          <Button onClick={() => void onSave()} disabled={!path}>{t("projects.save")}</Button>
        </div>
      </div>
    </div>
  );
};
