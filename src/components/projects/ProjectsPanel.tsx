import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderGit2, Trash2, Pencil, Link2, Link2Off, Pause, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ListItemRow } from "@/components/common/ListItemRow";
import {
  useProjects,
  useDeleteProject,
  useSetProjectEnabled,
  useApplyProject,
  useDetachProject,
} from "@/hooks/useProjects";
import type { Project } from "@/lib/api/projects";
import { toast } from "sonner";
import { ProjectBindDialog } from "./ProjectBindDialog";

export interface ProjectsPanelHandle {
  openCreate(): void;
}
interface ProjectsPanelProps {
  currentApp?: string;
}

const ProjectsPanel = React.forwardRef<ProjectsPanelHandle, ProjectsPanelProps>(
  (props, ref) => {
    const { t } = useTranslation();
    const { currentApp } = props;
    const { data: projects, isLoading } = useProjects();
    const del = useDeleteProject();
    const setEnabled = useSetProjectEnabled();
    const applyM = useApplyProject();
    const detachM = useDetachProject();

    const [dialogOpen, setDialogOpen] = useState(false);
    const [editing, setEditing] = useState<Project | null>(null);
    const [confirm, setConfirm] = useState<{ title: string; message: string; onConfirm: () => void } | null>(null);

    React.useImperativeHandle(ref, () => ({
      openCreate() {
        setEditing(null);
        setDialogOpen(true);
      },
    }));

    const onApply = async (p: Project) => {
      try {
        const r = await applyM.mutateAsync(p.id);
        if (r.warnings.length) r.warnings.forEach((w) => toast.warning(w));
        else toast.success(t("projects.applySuccess", { name: p.name ?? p.enteredPath }));
      } catch (e) {
        toast.error(t("common.error"), { description: String(e) });
      }
    };
    const onDetach = async (p: Project) => {
      try {
        await detachM.mutateAsync(p.id);
        toast.success(t("projects.detachSuccess", { name: p.name ?? p.enteredPath }));
      } catch (e) {
        toast.error(t("common.error"), { description: String(e) });
      }
    };

    return (
      <div className="px-6 flex flex-col flex-1 min-h-0 overflow-hidden">
        <div className="flex items-center justify-between py-2">
          <span className="text-sm text-muted-foreground">
            {t("projects.count", { count: projects?.length ?? 0 })}
          </span>
        </div>
        <div className="flex-1 overflow-y-auto overflow-x-hidden pb-24">
          {isLoading ? (
            <div className="text-center py-12 text-muted-foreground">{t("common.loading")}</div>
          ) : !projects || projects.length === 0 ? (
            <div className="text-center py-12">
              <div className="w-16 h-16 mx-auto mb-4 bg-muted rounded-full flex items-center justify-center">
                <FolderGit2 size={24} className="text-muted-foreground" />
              </div>
              <h3 className="text-lg font-medium text-foreground mb-2">{t("projects.empty")}</h3>
              <p className="text-muted-foreground text-sm">{t("projects.emptyDescription")}</p>
            </div>
          ) : (
            <TooltipProvider delayDuration={300}>
              <div className="rounded-xl border border-border-default overflow-hidden">
                {projects.map((p, i) => (
                  <ListItemRow key={p.id} isLast={i === projects.length - 1}>
                    <div className="flex-1 min-w-0">
                      <span className="font-medium text-sm text-foreground truncate block">
                        {p.name ?? p.enteredPath}
                      </span>
                      <p className="text-xs text-muted-foreground truncate" title={p.projectPath}>
                        {p.enteredPath}
                      </p>
                    </div>
                    <div className="flex-shrink-0 flex items-center gap-2">
                      <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => void onApply(p)} disabled={!p.enabled} title={t("projects.apply")}>
                        <Link2 size={14} className="mr-1" /> {t("projects.apply")}
                      </Button>
                      <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => void onDetach(p)} title={t("projects.detach")}>
                        <Link2Off size={14} className="mr-1" /> {t("projects.detach")}
                      </Button>
                      <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => void setEnabled.mutateAsync({ id: p.id, enabled: !p.enabled })} title={p.enabled ? t("projects.pause") : t("projects.resume")}>
                        {p.enabled ? <Pause size={14} /> : <Play size={14} />}
                      </Button>
                      <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => { setEditing(p); setDialogOpen(true); }} title={t("projects.edit")}>
                        <Pencil size={14} />
                      </Button>
                      <Button variant="ghost" size="icon" className="h-7 w-7 hover:text-red-500" onClick={() => setConfirm({ title: t("projects.delete"), message: t("projects.deleteConfirmDescription", { name: p.name ?? p.enteredPath }), onConfirm: async () => { await del.mutateAsync(p.id); setConfirm(null); toast.success(t("projects.deleteSuccess", { name: p.name ?? p.enteredPath })); } })} title={t("projects.delete")}>
                        <Trash2 size={14} />
                      </Button>
                    </div>
                  </ListItemRow>
                ))}
              </div>
            </TooltipProvider>
          )}
        </div>
        {confirm && (
          <ConfirmDialog isOpen title={confirm.title} message={confirm.message} variant="destructive" zIndex="top" onConfirm={() => void confirm.onConfirm()} onCancel={() => setConfirm(null)} />
        )}
        <ProjectBindDialog open={dialogOpen} project={editing} currentApp={currentApp ?? "claude"} onClose={() => { setDialogOpen(false); setEditing(null); }} />
      </div>
    );
  },
);
ProjectsPanel.displayName = "ProjectsPanel";
export default ProjectsPanel;
