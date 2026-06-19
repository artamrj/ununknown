import type { SettingsSectionProps } from "@/features/settings/types";
import { NumberField, Section, Toggle } from "@/features/settings/components/SettingsFields";

export function ExpertSettings({ settings, visible, set, nested }: SettingsSectionProps) {
  return (
    <>
      <Section
        title="Temporary data"
        note="Matched preview results survive restart only for this many days. New scans clear them immediately."
      >
        <NumberField
          show={visible}
          l="Matched preview retention days"
          d="Keep successful matched previews for restart recovery."
          v={settings.workspace_retention_days}
          set={(value) => set("workspace_retention_days", value)}
        />
      </Section>
      <Section
        title="Dangerous file operations"
        note="Enable Expert Mode before editing originals, renaming files, or replacing existing cover art."
      >
        <Toggle
          show={visible}
          l="Enable Expert Mode"
          d="Unlock destructive file and replacement controls."
          v={settings.expert_mode}
          set={(value) => set("expert_mode", value)}
        />
        <div className={!settings.expert_mode ? "locked" : ""}>
          <Toggle
            show={visible}
            l="Write tags in-place"
            d="Allow writing directly to original files."
            v={settings.in_place.write_tags}
            set={(value) => nested("in_place", "write_tags", value)}
          />
          <Toggle
            show={visible}
            l="Embed cover art in-place"
            d="Embed downloaded artwork into originals."
            v={settings.in_place.embed_cover_art}
            set={(value) => nested("in_place", "embed_cover_art", value)}
          />
          <Toggle
            show={visible}
            l="Rename files"
            d="Rename originals after successful tag writing."
            v={settings.in_place.rename_files}
            set={(value) => nested("in_place", "rename_files", value)}
          />
          <Toggle
            show={visible}
            l="Reorganize folders"
            d="Move originals under generated folders."
            v={settings.in_place.rename_folders}
            set={(value) => nested("in_place", "rename_folders", value)}
          />
          <Toggle
            show={visible}
            l="Preserve modification time"
            d="Restore original modification time after writes."
            v={settings.in_place.preserve_mtime}
            set={(value) => nested("in_place", "preserve_mtime", value)}
          />
        </div>
      </Section>
    </>
  );
}
