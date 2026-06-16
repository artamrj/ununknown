import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { Button } from "../components";
import {
  Choice,
  Field,
  NumberField,
  Reset,
  Section,
  Select,
  TemplateField,
  Toggle,
  ToggleCard,
  metadataGroups,
  modeHelp,
  tabs,
} from "../features/settings";

export function SettingsPage({ settings, back }: { settings: any; back: () => void }) {
  const q = useQueryClient();
  const [f, setF] = useState<any>({ ...settings });
  const [tab, setTab] = useState("Basic");
  const [search, setSearch] = useState("");
  const [msg, setMsg] = useState("");
  const [dirty, setDirty] = useState(false);
  const change = (v: any) => {
    setF(v);
    setDirty(true);
  };
  const set = (k: string, v: any) => change({ ...f, [k]: v });
  const nested = (s: string, k: string, v: any) => change({ ...f, [s]: { ...f[s], [k]: v } });
  const save = useMutation({
    mutationFn: () => api("/settings", { method: "PUT", body: JSON.stringify(f) }),
    onSuccess: () => {
      setMsg("Settings saved");
      setDirty(false);
      q.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (e) => setMsg(e.message),
  });
  const reset = (section?: string) =>
    api(`/settings/reset${section ? `/${section}` : ""}`, { method: "POST" }).then(() =>
      location.reload(),
    );
  const visible = (text: string) => !search || text.toLowerCase().includes(search.toLowerCase());
  const pathPreview = useQuery({
    queryKey: ["path-preview", f],
    queryFn: () =>
      api<any>("/path-template/preview", { method: "POST", body: JSON.stringify({ settings: f }) }),
    enabled: tab === "Files & Paths",
  });
  useEffect(() => {
    if (!settings) return;
    setF({ ...settings });
  }, [settings]);
  const content: any = {
    Basic: (
      <>
        <Section
          title="Folders"
          note="These are container paths from Docker Compose. Keep them unless you changed mounts."
        >
          <Field
            show={visible}
            l="Input folder"
            d="Scanned recursively for audio files."
            v={f.input_dir}
            set={(v) => set("input_dir", v)}
          />
          <Field
            show={visible}
            l="Output folder"
            d="Where copied and corrected files are written."
            v={f.output_dir}
            set={(v) => set("output_dir", v)}
          />
        </Section>
        <Section
          title="Writing mode"
          note="Copy mode is safest. In-place mode requires Expert Mode because it edits originals."
        >
          <Choice
            value={f.output_mode}
            set={(v) => set("output_mode", v)}
            items={[
              ["copy", "Copy to output", "Leave input files unchanged."],
              ["in_place", "In-place", "Write directly to originals. Requires Expert Mode."],
            ]}
          />
          <Toggle
            show={visible}
            l="Download cover art"
            d="Fetch artwork from Cover Art Archive when a release is known."
            v={f.cover_art_enabled}
            set={(v) => set("cover_art_enabled", v)}
          />
        </Section>
      </>
    ),
    Matching: (
      <>
        <Section
          title="Automatic selection"
          note="Ununknown 0.3 previews only automatically selected matches. Lower scores are counted as unmatched and not saved."
        >
          <Choice
            value={f.automation_mode}
            set={(v) => set("automation_mode", v)}
            items={Object.entries(modeHelp).map(([k, v]: any) => [k, v[0], v[1]])}
          />
          {f.automation_mode === "custom" && (
            <NumberField
              show={visible}
              l="Custom confidence threshold"
              d="Matches at or above this score are selected for Preview."
              v={f.confidence_threshold}
              set={(v) => set("confidence_threshold", v)}
            />
          )}
          <NumberField
            show={visible}
            l="Attempts per track"
            d="Retry the complete identification process before moving to the next file."
            v={f.track_attempts}
            set={(v) => set("track_attempts", v)}
          />
        </Section>
        <Reset onClick={() => reset("matching")} />
      </>
    ),
    Metadata: (
      <>
        <Section
          title="Write behavior"
          note="When overwrite is off, Ununknown fills only missing enabled fields."
        >
          <Toggle
            show={visible}
            l="Overwrite existing tags"
            d="Replace existing values for enabled metadata fields."
            v={f.overwrite_existing_tags}
            set={(v) => set("overwrite_existing_tags", v)}
          />
        </Section>
        {metadataGroups.map(([title, fields]: any) => (
          <Section title={title} note="" key={title}>
            <div className="toggle-card-grid">
              {fields.map(([key, label, desc]: any) => (
                <ToggleCard
                  key={key}
                  show={visible}
                  label={label}
                  desc={desc}
                  checked={Boolean(f.metadata_fields?.[key])}
                  onChange={(v) => nested("metadata_fields", key, v)}
                />
              ))}
            </div>
          </Section>
        ))}
        <Reset onClick={() => reset("metadata")} />
      </>
    ),
    "Files & Paths": (
      <>
        <Section
          title="Template editor"
          note="Templates are relative paths. Ununknown preserves the original extension automatically and blocks path traversal."
        >
          <TemplateField
            label="Output template"
            desc="Used for normal copied files and folder reorganization."
            value={f.path_templates.default_template}
            onChange={(v) => nested("path_templates", "default_template", v)}
          />
          <TemplateField
            label="Compilation template"
            desc="Used when album artist is Various Artists."
            value={f.path_templates.compilation_template}
            onChange={(v) => nested("path_templates", "compilation_template", v)}
          />
          <TemplateField
            label="In-place filename template"
            desc="Used only when in-place rename is enabled without folder reorganization."
            value={f.in_place.filename_template}
            onChange={(v) => nested("in_place", "filename_template", v)}
          />
        </Section>
        <Section
          title="Live examples"
          note="These update from your current draft settings before you save."
        >
          <div className="examples">
            {pathPreview.isLoading ? (
              <p>Rendering examples…</p>
            ) : (
              pathPreview.data?.examples?.map((x: any) => (
                <div className={`example ${x.errors?.length ? "bad" : ""}`} key={x.label}>
                  <span>{x.label}</span>
                  <code>{x.template}</code>
                  <strong>{x.path || "Invalid template"}</strong>
                  {x.errors?.map((e: string) => (
                    <small key={e}>{e}</small>
                  ))}
                  {x.warnings?.map((e: string) => (
                    <small key={e}>{e}</small>
                  ))}
                </div>
              ))
            )}
          </div>
          <div className="template-cheatsheet">
            {[
              "$title",
              "$artist/$title",
              "$artist/$year - $title",
              "$albumartist/$album/$track - $title",
            ].map((x) => (
              <button
                type="button"
                onClick={() => nested("path_templates", "default_template", x)}
                key={x}
              >
                {x}
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
            v={f.path_templates.collision_strategy}
            set={(v) => nested("path_templates", "collision_strategy", v)}
            o={["skip", "rename", "overwrite"]}
          />
          <Field
            show={visible}
            l="Unknown artist fallback"
            d="Used when artist is missing."
            v={f.path_templates.unknown_artist}
            set={(v) => nested("path_templates", "unknown_artist", v)}
          />
          <Field
            show={visible}
            l="Unknown album fallback"
            d="Used when album is missing."
            v={f.path_templates.unknown_album}
            set={(v) => nested("path_templates", "unknown_album", v)}
          />
          <Field
            show={visible}
            l="Unknown title fallback"
            d="Used when title is missing."
            v={f.path_templates.unknown_title}
            set={(v) => nested("path_templates", "unknown_title", v)}
          />
          <NumberField
            show={visible}
            l="Track padding"
            d="Digits used for track numbers."
            v={f.path_templates.track_padding}
            set={(v) => nested("path_templates", "track_padding", v)}
          />
          <NumberField
            show={visible}
            l="Disc padding"
            d="Digits used for disc numbers."
            v={f.path_templates.disc_padding}
            set={(v) => nested("path_templates", "disc_padding", v)}
          />
          <NumberField
            show={visible}
            l="Maximum filename length"
            d="Maximum characters per generated filename."
            v={f.path_templates.max_filename_length}
            set={(v) => nested("path_templates", "max_filename_length", v)}
          />
        </Section>
        <Reset onClick={() => reset("files")} />
      </>
    ),
    Expert: (
      <>
        <Section
          title="Temporary data"
          note="Matched preview results survive restart only for this many days. New scans clear them immediately."
        >
          <NumberField
            show={visible}
            l="Matched preview retention days"
            d="Keep successful matched previews for restart recovery."
            v={f.workspace_retention_days}
            set={(v) => set("workspace_retention_days", v)}
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
            v={f.expert_mode}
            set={(v) => set("expert_mode", v)}
          />
          <div className={!f.expert_mode ? "locked" : ""}>
            <Toggle
              show={visible}
              l="Write tags in-place"
              d="Allow writing directly to original files."
              v={f.in_place.write_tags}
              set={(v) => nested("in_place", "write_tags", v)}
            />
            <Toggle
              show={visible}
              l="Embed cover art in-place"
              d="Embed downloaded artwork into originals."
              v={f.in_place.embed_cover_art}
              set={(v) => nested("in_place", "embed_cover_art", v)}
            />
            <Toggle
              show={visible}
              l="Rename files"
              d="Rename originals after successful tag writing."
              v={f.in_place.rename_files}
              set={(v) => nested("in_place", "rename_files", v)}
            />
            <Toggle
              show={visible}
              l="Reorganize folders"
              d="Move originals under generated folders."
              v={f.in_place.rename_folders}
              set={(v) => nested("in_place", "rename_folders", v)}
            />
            <Toggle
              show={visible}
              l="Preserve modification time"
              d="Restore original modification time after writes."
              v={f.in_place.preserve_mtime}
              set={(v) => nested("in_place", "preserve_mtime", v)}
            />
          </div>
        </Section>
      </>
    ),
  };
  return (
    <main className="settings-page new-settings">
      <header>
        <div>
          <button
            className="back"
            onClick={() => (!dirty || confirm("Discard unsaved changes?") ? back() : null)}
          >
            ← Workspace
          </button>
          <h1>Settings</h1>
          <p>{dirty ? "Unsaved changes" : "Configure matching, metadata, and output paths."}</p>
        </div>
        <div>
          <Button
            kind="quiet"
            onClick={() =>
              confirm("Reset all settings? Paths and secrets are preserved.") && reset()
            }
          >
            Reset all
          </Button>
          <Button
            onClick={() =>
              (f.expert_mode &&
                confirm("Save Expert Mode settings? These can modify original files.")) ||
              !f.expert_mode
                ? save.mutate()
                : null
            }
          >
            Save settings
          </Button>
        </div>
      </header>
      {msg && <div className="message">{msg}</div>}
      <input
        className="settings-search"
        placeholder="Search settings…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <nav className="settings-tabs">
        {tabs.map((x) => (
          <button className={tab === x ? "active" : ""} onClick={() => setTab(x)} key={x}>
            {x}
          </button>
        ))}
      </nav>
      <section className="settings-stack">{content[tab]}</section>
    </main>
  );
}
