import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Terminal, Trash2, Pencil } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ListItemRow } from "@/components/common/ListItemRow";
import {
  useInstalledCommands,
  useSetCommandEnabled,
  useDeleteCommand,
} from "@/hooks/useCommands";
import type { InstalledCommand } from "@/lib/api/commands";
import { toast } from "sonner";
import { CommandEditDialog } from "./CommandEditDialog";

export interface CommandsPanelHandle {
  openCreate(): void;
}

interface CommandsPanelProps {
  currentApp?: string;
}

const CommandsPanel = React.forwardRef<CommandsPanelHandle, CommandsPanelProps>(
  (_props, ref) => {
    const { t } = useTranslation();

    const { data: commands, isLoading } = useInstalledCommands();
    const setEnabledMutation = useSetCommandEnabled();
    const deleteMutation = useDeleteCommand();

    const [editDialogOpen, setEditDialogOpen] = useState(false);
    const [editingCommand, setEditingCommand] =
      useState<InstalledCommand | null>(null);

    const [confirmDialog, setConfirmDialog] = useState<{
      isOpen: boolean;
      title: string;
      message: string;
      onConfirm: () => void;
    } | null>(null);

    React.useImperativeHandle(ref, () => ({
      openCreate() {
        setEditingCommand(null);
        setEditDialogOpen(true);
      },
    }));

    const handleToggleEnabled = async (
      command: InstalledCommand,
      enabled: boolean,
    ) => {
      try {
        await setEnabledMutation.mutateAsync({ id: command.id, enabled });
      } catch (error) {
        toast.error(t("common.error"), { description: String(error) });
      }
    };

    const handleEdit = (command: InstalledCommand) => {
      setEditingCommand(command);
      setEditDialogOpen(true);
    };

    const handleDelete = (command: InstalledCommand) => {
      setConfirmDialog({
        isOpen: true,
        title: t("commands.delete"),
        message: t("commands.deleteConfirmDescription", { name: command.name }),
        onConfirm: async () => {
          try {
            await deleteMutation.mutateAsync(command.id);
            setConfirmDialog(null);
            toast.success(t("commands.deleteSuccess", { name: command.name }));
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
            {t("commands.count", { count: commands?.length ?? 0 })}
          </span>
        </div>

        {/* List */}
        <div className="flex-1 overflow-y-auto overflow-x-hidden pb-24">
          {isLoading ? (
            <div className="text-center py-12 text-muted-foreground">
              {t("common.loading")}
            </div>
          ) : !commands || commands.length === 0 ? (
            <div className="text-center py-12">
              <div className="w-16 h-16 mx-auto mb-4 bg-muted rounded-full flex items-center justify-center">
                <Terminal size={24} className="text-muted-foreground" />
              </div>
              <h3 className="text-lg font-medium text-foreground mb-2">
                {t("commands.empty")}
              </h3>
              <p className="text-muted-foreground text-sm">
                {t("commands.emptyDescription")}
              </p>
            </div>
          ) : (
            <TooltipProvider delayDuration={300}>
              <div className="rounded-xl border border-border-default overflow-hidden">
                {commands.map((command, index) => (
                  <CommandListItem
                    key={command.id}
                    command={command}
                    isLast={index === commands.length - 1}
                    onToggleEnabled={(enabled) =>
                      void handleToggleEnabled(command, enabled)
                    }
                    onEdit={() => handleEdit(command)}
                    onDelete={() => handleDelete(command)}
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
        <CommandEditDialog
          open={editDialogOpen}
          command={editingCommand}
          onClose={() => {
            setEditDialogOpen(false);
            setEditingCommand(null);
          }}
        />
      </div>
    );
  },
);

CommandsPanel.displayName = "CommandsPanel";

// ========== List item ==========

interface CommandListItemProps {
  command: InstalledCommand;
  isLast?: boolean;
  onToggleEnabled: (enabled: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}

const CommandListItem: React.FC<CommandListItemProps> = ({
  command,
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
          /{command.name}
        </span>
        {command.description && (
          <p
            className="text-xs text-muted-foreground truncate"
            title={command.description}
          >
            {command.description}
          </p>
        )}
      </div>

      {/* Right: enable switch + action buttons */}
      <div className="flex-shrink-0 flex items-center gap-2">
        <Switch
          checked={command.enabledClaude}
          onCheckedChange={onToggleEnabled}
          title={t("commands.enabledClaude")}
          aria-label={t("commands.enabledClaude")}
        />

        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-blue-500 hover:bg-blue-100 dark:hover:text-blue-400 dark:hover:bg-blue-500/10"
            onClick={onEdit}
            title={t("commands.edit")}
          >
            <Pencil size={14} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-red-500 hover:bg-red-100 dark:hover:text-red-400 dark:hover:bg-red-500/10"
            onClick={onDelete}
            title={t("commands.delete")}
          >
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
    </ListItemRow>
  );
};

export default CommandsPanel;
