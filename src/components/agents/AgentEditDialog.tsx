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
import { useCreateAgent, useUpdateAgent } from "@/hooks/useAgents";
import type { InstalledAgent } from "@/lib/api/agents";
import { toast } from "sonner";

const NAME_RE = /^[A-Za-z0-9._-]+$/;

interface AgentEditDialogProps {
  open: boolean;
  /** When provided the dialog is in edit mode; otherwise create mode */
  agent?: InstalledAgent | null;
  onClose: () => void;
}

export const AgentEditDialog: React.FC<AgentEditDialogProps> = ({
  open,
  agent,
  onClose,
}) => {
  const { t } = useTranslation();
  const isEdit = Boolean(agent);

  const [name, setName] = useState("");
  const [content, setContent] = useState("");
  const [description, setDescription] = useState("");
  const [tags, setTags] = useState("");
  const [nameError, setNameError] = useState("");

  const createMutation = useCreateAgent();
  const updateMutation = useUpdateAgent();
  const isPending = createMutation.isPending || updateMutation.isPending;

  // Sync form when dialog opens
  useEffect(() => {
    if (open) {
      if (agent) {
        setName(agent.name);
        setContent(agent.content);
        setDescription(agent.description ?? "");
        setTags(agent.tags.join(", "));
      } else {
        setName("");
        setContent("");
        setDescription("");
        setTags("");
      }
      setNameError("");
    }
  }, [open, agent]);

  const parsedTags = tags
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    if (!isEdit) {
      if (!NAME_RE.test(name)) {
        setNameError(t("agents.nameInvalid"));
        return;
      }
    }

    try {
      if (isEdit && agent) {
        await updateMutation.mutateAsync({
          id: agent.id,
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
        toast.success(t("agents.createSuccess", { name }));
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
            {isEdit ? t("agents.edit") : t("agents.create")}
          </DialogTitle>
        </DialogHeader>

        <form onSubmit={(e) => void handleSubmit(e)} className="space-y-4 py-2">
          {!isEdit && (
            <div className="space-y-1">
              <label className="text-sm font-medium">{t("agents.name")}</label>
              <input
                type="text"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setNameError("");
                }}
                placeholder={t("agents.namePlaceholder")}
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
            <label className="text-sm font-medium">{t("agents.content")}</label>
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              placeholder={t("agents.contentPlaceholder")}
              rows={10}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm font-mono focus:outline-none focus:ring-2 focus:ring-ring resize-y"
              required
            />
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium">
              {t("agents.description")}
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("agents.descriptionPlaceholder")}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>

          <div className="space-y-1">
            <label className="text-sm font-medium">{t("agents.tags")}</label>
            <input
              type="text"
              value={tags}
              onChange={(e) => setTags(e.target.value)}
              placeholder={t("agents.tagsPlaceholder")}
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
              {t("agents.cancel")}
            </Button>
            <Button type="submit" disabled={isPending}>
              {t("agents.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default AgentEditDialog;
