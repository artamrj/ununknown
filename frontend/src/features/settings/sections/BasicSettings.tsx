import type { SettingsSectionProps } from "@/features/settings/types";
import { Choice, Field, Section, Toggle } from "@/features/settings/components/SettingsFields";

export function BasicSettings({ settings, visible, set }: SettingsSectionProps) {
  return (
    <>
      <Section
        title="Folders"
        note="These are container paths from Docker Compose. Keep them unless you changed mounts."
      >
        <Field
          show={visible}
          l="Input folder"
          d="Scanned recursively for audio files."
          v={settings.input_dir}
          set={(value) => set("input_dir", value)}
        />
        <Field
          show={visible}
          l="Output folder"
          d="Where copied and corrected files are written."
          v={settings.output_dir}
          set={(value) => set("output_dir", value)}
        />
      </Section>
      <Section
        title="Writing mode"
        note="Copy mode is safest. In-place mode requires Expert Mode because it edits originals."
      >
        <Choice
          value={settings.output_mode}
          set={(value) => set("output_mode", value)}
          items={[
            ["copy", "Copy to output", "Leave input files unchanged."],
            ["in_place", "In-place", "Write directly to originals. Requires Expert Mode."],
          ]}
        />
        <Toggle
          show={visible}
          l="Download cover art"
          d="Fetch artwork from Cover Art Archive when a release is known."
          v={settings.cover_art_enabled}
          set={(value) => set("cover_art_enabled", value)}
        />
      </Section>
    </>
  );
}
