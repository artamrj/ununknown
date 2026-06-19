import type { FilesAndPathsSettingsProps } from "@/features/settings/types";
import {
  Field,
  NumberField,
  Reset,
  Section,
  Select,
  TemplateField,
} from "@/features/settings/components/SettingsFields";

const templateCheatsheet = [
  "$title",
  "$artist/$title",
  "$artist/$year - $title",
  "$albumartist/$album/$track - $title",
];

export function FilesAndPathsSettings({
  settings,
  visible,
  nested,
  reset,
  pathPreview,
}: FilesAndPathsSettingsProps) {
  return (
    <>
      <Section
        title="Template editor"
        note="Templates are relative paths. Ununknown preserves the original extension automatically and blocks path traversal."
      >
        <TemplateField
          label="Output template"
          desc="Used for normal copied files and folder reorganization."
          value={settings.path_templates.default_template}
          onChange={(value) => nested("path_templates", "default_template", value)}
        />
        <TemplateField
          label="Compilation template"
          desc="Used when album artist is Various Artists."
          value={settings.path_templates.compilation_template}
          onChange={(value) => nested("path_templates", "compilation_template", value)}
        />
        <TemplateField
          label="In-place filename template"
          desc="Used only when in-place rename is enabled without folder reorganization."
          value={settings.in_place.filename_template}
          onChange={(value) => nested("in_place", "filename_template", value)}
        />
      </Section>
      <Section title="Live examples" note="These update from your current draft settings before you save.">
        <div className="examples">
          {pathPreview.isLoading ? (
            <p>Rendering examples...</p>
          ) : (
            pathPreview.data?.examples?.map((example) => (
              <div className={`example ${example.errors?.length ? "bad" : ""}`} key={example.label}>
                <span>{example.label}</span>
                <code>{example.template}</code>
                <strong>{example.path || "Invalid template"}</strong>
                {example.errors?.map((error) => (
                  <small key={error}>{error}</small>
                ))}
                {example.warnings?.map((warning) => (
                  <small key={warning}>{warning}</small>
                ))}
              </div>
            ))
          )}
        </div>
        <div className="template-cheatsheet">
          {templateCheatsheet.map((template) => (
            <button
              type="button"
              onClick={() => nested("path_templates", "default_template", template)}
              key={template}
            >
              {template}
            </button>
          ))}
        </div>
      </Section>
      <Section
        title="Fallbacks and safety"
        note="Fallbacks are used when metadata is missing. Collision behavior controls what happens when an output path already exists."
      >
        <Select
          show={visible}
          l="Collision strategy"
          d="Skip, numbered rename, or overwrite."
          v={settings.path_templates.collision_strategy}
          set={(value) => nested("path_templates", "collision_strategy", value)}
          o={["skip", "rename", "overwrite"]}
        />
        <Field
          show={visible}
          l="Unknown artist fallback"
          d="Used when artist is missing."
          v={settings.path_templates.unknown_artist}
          set={(value) => nested("path_templates", "unknown_artist", value)}
        />
        <Field
          show={visible}
          l="Unknown album fallback"
          d="Used when album is missing."
          v={settings.path_templates.unknown_album}
          set={(value) => nested("path_templates", "unknown_album", value)}
        />
        <Field
          show={visible}
          l="Unknown title fallback"
          d="Used when title is missing."
          v={settings.path_templates.unknown_title}
          set={(value) => nested("path_templates", "unknown_title", value)}
        />
        <NumberField
          show={visible}
          l="Track padding"
          d="Digits used for track numbers."
          v={settings.path_templates.track_padding}
          set={(value) => nested("path_templates", "track_padding", value)}
        />
        <NumberField
          show={visible}
          l="Disc padding"
          d="Digits used for disc numbers."
          v={settings.path_templates.disc_padding}
          set={(value) => nested("path_templates", "disc_padding", value)}
        />
        <NumberField
          show={visible}
          l="Maximum filename length"
          d="Maximum characters per generated filename."
          v={settings.path_templates.max_filename_length}
          set={(value) => nested("path_templates", "max_filename_length", value)}
        />
      </Section>
      <Reset onClick={() => reset("files")} />
    </>
  );
}
