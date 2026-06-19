import type { SettingsSectionProps } from "@/features/settings/types";
import { Choice, NumberField, Reset, Section } from "@/features/settings/components/SettingsFields";
import { compilationHelp, modeHelp } from "@/features/settings/settingsOptions";

export function MatchingSettings({ settings, visible, set, reset }: SettingsSectionProps) {
  return (
    <>
      <Section
        title="Automatic selection"
        note="Ununknown 0.3 previews only automatically selected matches. Lower scores are counted as unmatched and not saved."
      >
        <Choice
          value={settings.automation_mode}
          set={(value) => set("automation_mode", value)}
          items={Object.entries(modeHelp).map(([key, value]) => [key, value[0], value[1]])}
        />
        {settings.automation_mode === "custom" && (
          <NumberField
            show={visible}
            l="Custom confidence threshold"
            d="Matches at or above this score are selected for Preview."
            v={settings.confidence_threshold}
            set={(value) => set("confidence_threshold", value)}
          />
        )}
        <Choice
          value={settings.compilation_preference || "avoid"}
          set={(value) => set("compilation_preference", value)}
          items={Object.entries(compilationHelp).map(([key, value]) => [key, value[0], value[1]])}
        />
        <NumberField
          show={visible}
          l="Attempts per track"
          d="Retry the complete identification process before moving to the next file."
          v={settings.track_attempts}
          set={(value) => set("track_attempts", value)}
        />
        <NumberField
          show={visible}
          l="Metadata read workers"
          d="Parallel tag and audio property readers."
          v={settings.metadata_read_concurrency}
          set={(value) => set("metadata_read_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="Scan workers"
          d="Maximum tracks processed through the scan pipeline at once."
          v={settings.scan_worker_concurrency}
          set={(value) => set("scan_worker_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="Fingerprint workers"
          d="Parallel fpcalc processes for local audio fingerprints."
          v={settings.fingerprint_concurrency}
          set={(value) => set("fingerprint_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="AcoustID lookups"
          d="Parallel AcoustID requests before MusicBrainz enrichment."
          v={settings.acoustid_concurrency}
          set={(value) => set("acoustid_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="Artwork downloads"
          d="Parallel cover-art downloads during preview and apply flows."
          v={settings.artwork_download_concurrency}
          set={(value) => set("artwork_download_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="Tag writers"
          d="Parallel blocking metadata writes during apply."
          v={settings.tag_write_concurrency}
          set={(value) => set("tag_write_concurrency", value)}
        />
        <NumberField
          show={visible}
          l="DB write batch size"
          d="Selected matches grouped per persistence transaction."
          v={settings.db_write_batch_size}
          set={(value) => set("db_write_batch_size", value)}
        />
      </Section>
      <Reset onClick={() => reset("matching")} />
    </>
  );
}
