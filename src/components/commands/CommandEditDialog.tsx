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
import { useCreateCommand, useUpdateCommand } from "@/hooks/useCommands";
import type { InstalledCommand } from "@/lib/api/commands";
import { toast } from "sonner";

const NAME_RE = /^[A-Za-z0-9._-]+$/;

interface CommandEditDialogProps {
  open: boolean;
  /** When provided the dialog is in edit mode; otherwise create mode */
  command?: InstalledCommand | null;
  onClose: () => void;
}

export const CommandEditDialog: React.FC<CommandEditDialogProps> = ({
  open,
  command,
  onClose,
}) => {
  const { t } = useTranslation();
  const isEdit = Boolean(command);

  const [name, setName] = useState("");
  const [content, setContent] = useState("");
  const [description, setDescription] = useState("");
  const [tags, setTags] = useState("");
  const [nameError, setNameError] = useState("");

  const createMutation = useCreateCommand();
  const updateMutation = useUpdateCommand();
  const isPending = createMutation.isPending || updateMutation.isPending;

  // Sync form when dialog opens
  useEffect(() => {
    if (open) {
      if (command) {
        setName(command.name);
        setContent(command.content);
        setDescription(command.description ?? "");
        setTags(command.tags.join(", "));
      } else {
        setName("");
        setContent("");
        setDescription("");
        setTags("");
      }
      setNameError("");
    }
  }, [open, command]);

  const parsedTags = tags
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!isEdit) {
      if (!NAME_RE.test(name)) {
        setNameError(t("commands.nameInvalid"));
        return;
      }
    }

    try {
      if (isEdit && command) {
        await updateMutation.mutateAsync({
          id: command.id,
          content,
          description: description.trim() || null,
          tags: parsedTags,
        });
      } else {
        await createMutation.mutateAsync({
          name,
          content,
          description: description.trim() || null,
          tags: parsedTags,
        });
        toast.success(t("commands.createSuccess", { name }));
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
            {isEdit ? t("commands.edit") : t("commands.create")}
          </DialogTitle>
        </DialogHeader>

        <form onSubmit={(e) => void handleSubmit(e)} className="space-y-4 py-2">
          {!isEdit && (
            <div className="space-y-1">
              <label className="text-sm font-medium">
                {t("commands.name")}
              </label>
              <input
                type="text"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setNameError("");
                }}
                placeholder={t("commands.namePlaceholder")}
                className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
                required
                autoFocus
              />
              {nameError && (
                <p className="text-xs text-destructive">{nameError}</p>
              )}
            </div>
          )}

          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("commands.content")}
            </label>
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              placeholder={t("commands.contentPlaceholder")}
              rows={10}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-ring resize-y"
              required
            />
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("commands.description")}
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("commands.descriptionPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium">{t("commands.tags")}</label>
            <input
              type="text"
              value={tags}
              onChange={(e) => setTags(e.target.value)}
              placeholder={t("commands.tagsPlaceholder")}
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
              {t("commands.cancel")}
            </Button>
            <Button type="submit" disabled={isPending}>
              {t("commands.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default CommandEditDialog;
