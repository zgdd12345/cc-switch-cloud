import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { LayoutTemplate, Trash2, Pencil, CheckCircle2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ListItemRow } from "@/components/common/ListItemRow";
import {
  useInstalledProfiles,
  useDeleteProfile,
  useActivateProfile,
} from "@/hooks/useProfiles";
import type { InstalledProfile } from "@/lib/api/profiles";
import { toast } from "sonner";
import { ProfileEditDialog } from "./ProfileEditDialog";

export interface ProfilesPanelHandle {
  openCreate(): void;
}

interface ProfilesPanelProps {
  currentApp?: string;
}

const ProfilesPanel = React.forwardRef<ProfilesPanelHandle, ProfilesPanelProps>(
  (props, ref) => {
    const { t } = useTranslation();
    const { currentApp } = props;

    const { data: profiles, isLoading } = useInstalledProfiles();
    const deleteMutation = useDeleteProfile();
    const activateMutation = useActivateProfile();

    const [editDialogOpen, setEditDialogOpen] = useState(false);
    const [editingProfile, setEditingProfile] =
      useState<InstalledProfile | null>(null);

    const [confirmDialog, setConfirmDialog] = useState<{
      isOpen: boolean;
      title: string;
      message: string;
      onConfirm: () => void;
    } | null>(null);

    React.useImperativeHandle(ref, () => ({
      openCreate() {
        setEditingProfile(null);
        setEditDialogOpen(true);
      },
    }));

    const handleEdit = (profile: InstalledProfile) => {
      setEditingProfile(profile);
      setEditDialogOpen(true);
    };

    const handleDelete = (profile: InstalledProfile) => {
      setConfirmDialog({
        isOpen: true,
        title: t("profiles.delete"),
        message: t("profiles.deleteConfirmDescription", { name: profile.name }),
        onConfirm: async () => {
          try {
            await deleteMutation.mutateAsync(profile.id);
            setConfirmDialog(null);
            toast.success(t("profiles.deleteSuccess", { name: profile.name }));
          } catch (error) {
            toast.error(t("common.error"), { description: String(error) });
          }
        },
      });
    };

    const handleActivate = async (profile: InstalledProfile) => {
      const app = profile.appType ?? currentApp ?? "";
      try {
        const result = await activateMutation.mutateAsync({
          app,
          id: profile.id,
        });
        if (result.warnings && result.warnings.length > 0) {
          result.warnings.forEach((w) => toast.warning(w));
        } else {
          toast.success(t("profiles.activateSuccess", { name: profile.name }));
        }
      } catch (error) {
        toast.error(t("common.error"), { description: String(error) });
      }
    };

    return (
      <div className="px-6 flex flex-col flex-1 min-h-0 overflow-hidden">
        {/* Header count */}
        <div className="flex items-center justify-between py-2">
          <span className="text-sm text-muted-foreground">
            {t("profiles.count", { count: profiles?.length ?? 0 })}
          </span>
        </div>

        {/* List */}
        <div className="flex-1 overflow-y-auto overflow-x-hidden pb-24">
          {isLoading ? (
            <div className="text-center py-12 text-muted-foreground">
              {t("common.loading")}
            </div>
          ) : !profiles || profiles.length === 0 ? (
            <div className="text-center py-12">
              <div className="w-16 h-16 mx-auto mb-4 bg-muted rounded-full flex items-center justify-center">
                <LayoutTemplate size={24} className="text-muted-foreground" />
              </div>
              <h3 className="text-lg font-medium text-foreground mb-2">
                {t("profiles.empty")}
              </h3>
              <p className="text-muted-foreground text-sm">
                {t("profiles.emptyDescription")}
              </p>
            </div>
          ) : (
            <TooltipProvider delayDuration={300}>
              <div className="rounded-xl border border-border-default overflow-hidden">
                {profiles.map((profile, index) => (
                  <ProfileListItem
                    key={profile.id}
                    profile={profile}
                    isLast={index === profiles.length - 1}
                    onActivate={() => void handleActivate(profile)}
                    onEdit={() => handleEdit(profile)}
                    onDelete={() => handleDelete(profile)}
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
        <ProfileEditDialog
          open={editDialogOpen}
          profile={editingProfile}
          currentApp={currentApp}
          onClose={() => {
            setEditDialogOpen(false);
            setEditingProfile(null);
          }}
        />
      </div>
    );
  },
);

ProfilesPanel.displayName = "ProfilesPanel";

// ========== List item ==========

interface ProfileListItemProps {
  profile: InstalledProfile;
  isLast?: boolean;
  onActivate: () => void;
  onEdit: () => void;
  onDelete: () => void;
}

const ProfileListItem: React.FC<ProfileListItemProps> = ({
  profile,
  isLast,
  onActivate,
  onEdit,
  onDelete,
}) => {
  const { t } = useTranslation();

  return (
    <ListItemRow isLast={isLast}>
      {/* Left: name + description */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="font-medium text-sm text-foreground truncate block">
            {profile.name}
          </span>
          {profile.isActive && (
            <span
              className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium bg-green-100 text-green-700 dark:bg-green-500/20 dark:text-green-400"
              data-testid="active-badge"
            >
              <CheckCircle2 size={10} />
              {t("profiles.active")}
            </span>
          )}
        </div>
        {profile.description && (
          <p
            className="text-xs text-muted-foreground truncate"
            title={profile.description}
          >
            {profile.description}
          </p>
        )}
      </div>

      {/* Right: activate + action buttons */}
      <div className="flex-shrink-0 flex items-center gap-2">
        {!profile.isActive && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 text-xs"
            onClick={onActivate}
            title={t("profiles.activate")}
          >
            {t("profiles.activate")}
          </Button>
        )}

        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-blue-500 hover:bg-blue-100 dark:hover:text-blue-400 dark:hover:bg-blue-500/10"
            onClick={onEdit}
            title={t("profiles.edit")}
          >
            <Pencil size={14} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:text-red-500 hover:bg-red-100 dark:hover:text-red-400 dark:hover:bg-red-500/10"
            onClick={onDelete}
            title={t("profiles.delete")}
          >
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
    </ListItemRow>
  );
};

export default ProfilesPanel;
