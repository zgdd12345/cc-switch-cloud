import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Trash2, Pencil } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ListItemRow } from "@/components/common/ListItemRow";
import {
  useInstalledAgents,
  useSetAgentEnabled,
  useDeleteAgent,
} from "@/hooks/useAgents";
import type { InstalledAgent } from "@/lib/api/agents";
import { toast } from "sonner";
import { AgentEditDialog } from "./AgentEditDialog";

export interface AgentsPanelHandle {
  openCreate(): void;
}

interface AgentsPanelProps {
  currentApp?: string;
}

const AgentsPanel = React.forwardRef<AgentsPanelHandle, AgentsPanelProps>(
  (_props, ref) => {
    const { t } = useTranslation();

    const { data: agents, isLoading } = useInstalledAgents();
    const setEnabledMutation = useSetAgentEnabled();
    const deleteMutation = useDeleteAgent();

    const [editDialogOpen, setEditDialogOpen] = useState(false);
    const [editingAgent, setEditingAgent] = useState<InstalledAgent | null>(
      null,
    );

    const [confirmDialog, setConfirmDialog] = useState<{
      isOpen: boolean;
      title: string;
      message: string;
      onConfirm: () => void;
    } | null>(null);

    React.useImperativeHandle(ref, () => ({
      openCreate() {
        setEditingAgent(null);
        setEditDialogOpen(true);
      },
    }));

    const handleToggleEnabled = async (
      agent: InstalledAgent,
      enabled: boolean,
    ) => {
      try {
        await setEnabledMutation.mutateAsync({ id: agent.id, enabled });
      } catch (error) {
        toast.error(t("common.error"), { description: String(error) });
      }
    };

    const handleEdit = (agent: InstalledAgent) => {
      setEditingAgent(agent);
      setEditDialogOpen(true);
    };

    const handleDelete = (agent: InstalledAgent) => {
      setConfirmDialog({
        isOpen: true,
        title: t("agents.delete"),
        message: t("agents.deleteConfirmDescription", { name: agent.name }),
        onConfirm: async () => {
          try {
            await deleteMutation.mutateAsync(agent.id);
            setConfirmDialog(null);
            toast.success(t("agents.deleteSuccess", { name: agent.name }));
          } catch (error) {
            toast.error(t("common.error"), { description: String(error) });
          }
        },
      });
    };

    return (
      <div className="px-6 flex flex-col flex-1 min-h-0 overflow-hidden">
        {/* Header count */}
        <div className="flex items-center justify-between py-2">
          <span className="text-sm text-muted-foreground">
            {t("agents.count", { count: agents?.length ?? 0 })}
          </span>
        </div>

        {/* List */}
        <div className="flex-1 overflow-y-auto overflow-x-hidden pb-24">
          {isLoading ? (
            <div className="text-center py-12 text-muted-foreground">
              {t("common.loading")}
            </div>
          ) : !agents || agents.length === 0 ? (
            <div className="text-center py-12">
              <div className="w-16 h-16 mx-auto mb-4 bg-muted rounded-full flex items-center justify-center">
                <Bot size={24} className="text-muted-foreground" />
              </div>
              <h3 className="text-lg font-medium text-foreground mb-2">
                {t("agents.empty")}
              </h3>
              <p className="text-muted-foreground text-sm">
                {t("agents.emptyDescription")}
              </p>
            </div>
          ) : (
            <TooltipProvider delayDuration={300}>
              <div className="rounded-xl border border-border-default overflow-hidden">
                {agents.map((agent, index) => (
                  <AgentListItem
                    key={agent.id}
                    agent={agent}
                    isLast={index === agents.length - 1}
                    onToggleEnabled={(enabled) =>
                      void handleToggleEnabled(agent, enabled)
                    }
                    onEdit={() => handleEdit(agent)}
                    onDelete={() => handleDelete(agent)}
                  />
                ))}
              </div>
            </TooltipProvider>
          )}
        </div>

        {/* Confirm delete dialog */}
        {confirmDialog && (
          <ConfirmDialog
            isOpen={confirmDialog.isOpen}
            title={confirmDialog.title}
            message={confirmDialog.message}
            variant="destructive"
            zIndex="top"
            onConfirm={() => void confirmDialog.onConfirm()}
            onCancel={() => setConfirmDialog(null)}
          />
        )}

        {/* Create / edit dialog */}
        <AgentEditDialog
          open={editDialogOpen}
          agent={editingAgent}
          onClose={() => {
            setEditDialogOpen(false);
            setEditingAgent(null);
          }}
        />
      </div>
    );
  },
);

AgentsPanel.displayName = "AgentsPanel";

// ========== List item ==========

interface AgentListItemProps {
  agent: InstalledAgent;
  isLast?: boolean;
  onToggleEnabled: (enabled: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}

const AgentListItem: React.FC<AgentListItemProps> = ({
  agent,
  isLast,
  onToggleEnabled,
  onEdit,
  onDelete,
}) => {
  const { t } = useTranslation();

  return (
    <ListItemRow isLast={isLast}>
      {/* Left: name + description */}
      <div className="flex-1 min-w-0">
        <span className="font-medium text-sm text-foreground truncate block">
          {agent.name}
        </span>
        {agent.description && (
          <p
            className="text-xs text-muted-foreground truncate"
            title={agent.description}
          >
            {agent.description}
          </p>
        )}
      </div>

      {/* Right: enable switch + action buttons */}
      <div className="flex-shrink-0 flex items-center gap-2">
        <Switch
          checked={agent.enabledClaude}
          onCheckedChange={onToggleEnabled}
          title={t("agents.enabledClaude")}
          aria-label={t("agents.enabledClaude")}
        />

        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-blue-500 hover:bg-blue-100 dark:hover:text-blue-400 dark:hover:bg-blue-500/10"
            onClick={onEdit}
            title={t("agents.edit")}
          >
            <Pencil size={14} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-red-500 hover:bg-red-100 dark:hover:text-red-400 dark:hover:bg-red-500/10"
            onClick={onDelete}
            title={t("agents.delete")}
          >
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
    </ListItemRow>
  );
};

export default AgentsPanel;
