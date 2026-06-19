import type { SettingsSectionProps } from "@/features/settings/types";
import { Reset, Section, Toggle, ToggleCard } from "@/features/settings/components/SettingsFields";
import { metadataGroups } from "@/features/settings/settingsOptions";

export function MetadataSettings({ settings, visible, set, nested, reset }: SettingsSectionProps) {
  return (
    <>
      <Section
        title="Write behavior"
        note="When overwrite is off, Ununknown fills only missing enabled fields."
      >
        <Toggle
          show={visible}
          l="Overwrite existing tags"
          d="Replace existing values for enabled metadata fields."
          v={settings.overwrite_existing_tags}
          set={(value) => set("overwrite_existing_tags", value)}
        />
      </Section>
      {metadataGroups.map(([title, fields]) => (
        <Section title={title} note="" key={title}>
          <div className="toggle-card-grid">
            {fields.map(([key, label, desc]) => (
              <ToggleCard
                key={key}
                show={visible}
                label={label}
                desc={desc}
                checked={Boolean(settings.metadata_fields?.[key])}
                onChange={(value) => nested("metadata_fields", key, value)}
              />
            ))}
          </div>
        </Section>
      ))}
      <Reset onClick={() => reset("metadata")} />
    </>
  );
}
