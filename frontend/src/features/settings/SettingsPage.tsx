import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api";
import { BasicSettings } from "@/features/settings/sections/BasicSettings";
import { ExpertSettings } from "@/features/settings/sections/ExpertSettings";
import { FilesAndPathsSettings } from "@/features/settings/sections/FilesAndPathsSettings";
import { MatchingSettings } from "@/features/settings/sections/MatchingSettings";
import { MetadataSettings } from "@/features/settings/sections/MetadataSettings";
import { MetadataSourcesSettings } from "@/features/settings/sections/MetadataSourcesSettings";
import { JsonSettings } from "@/features/settings/sections/JsonSettings";
import { tabs } from "@/features/settings/settingsOptions";
import type { SettingsSectionProps } from "@/features/settings/types";
import { Button } from "@/shared/components/Button";

type SettingsPageProps = {
  settings: any;
  back: () => void;
  initialTab?: string;
};

export function SettingsPage({ settings, back, initialTab = "Basic" }: SettingsPageProps) {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<any>({ ...settings });
  const [tab, setTab] = useState(initialTab);
  const [search, setSearch] = useState("");
  const [msg, setMsg] = useState("");
  const [dirty, setDirty] = useState(false);
  const [jsonValid, setJsonValid] = useState(true);

  const change = (value: any) => {
    setDraft(value);
    setDirty(true);
  };
  const set = (key: string, value: any) => change({ ...draft, [key]: value });
  const nested = (section: string, key: string, value: any) =>
    change({ ...draft, [section]: { ...draft[section], [key]: value } });

  const save = useMutation({
    mutationFn: () => api("/settings", { method: "PUT", body: JSON.stringify(draft) }),
    onSuccess: () => {
      setMsg("Settings saved");
      setDirty(false);
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (error) => setMsg(error.message),
  });

  const reset = (section?: string) =>
    api(`/settings/reset${section ? `/${section}` : ""}`, { method: "POST" }).then(() =>
      location.reload(),
    );

  const visible = (text: string) => !search || text.toLowerCase().includes(search.toLowerCase());
  const pathPreview = useQuery({
    queryKey: ["path-preview", draft],
    queryFn: () =>
      api<any>("/path-template/preview", {
        method: "POST",
        body: JSON.stringify({ settings: draft }),
      }),
    enabled: tab === "Files & Paths",
  });

  useEffect(() => {
    if (!settings) return;
    setDraft({ ...settings });
  }, [settings]);

  const sectionProps: SettingsSectionProps = {
    settings: draft,
    visible,
    set,
    nested,
    reset,
  };

  const content = {
    Basic: <BasicSettings {...sectionProps} />,
    Matching: <MatchingSettings {...sectionProps} />,
    "Metadata Sources": <MetadataSourcesSettings {...sectionProps} />,
    Metadata: <MetadataSettings {...sectionProps} />,
    "Files & Paths": <FilesAndPathsSettings {...sectionProps} pathPreview={pathPreview} />,
    Expert: <ExpertSettings {...sectionProps} />,
    JSON: (
      <JsonSettings settings={draft} onChange={change} onValidityChange={setJsonValid} />
    ),
  };

  const canSave = jsonValid && !save.isPending;
  const handleSave = () => {
    if (!canSave) {
      setMsg("Fix JSON syntax before saving settings");
      return;
    }
    if (
      draft.expert_mode &&
      !confirm("Save Expert Mode settings? These can modify original files.")
    ) {
      return;
    }
    save.mutate();
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
          <Button onClick={handleSave} disabled={!canSave}>
            Save settings
          </Button>
        </div>
      </header>
      {msg && <div className="message">{msg}</div>}
      <input
        className="settings-search"
        placeholder="Search settings..."
        value={search}
        onChange={(event) => setSearch(event.target.value)}
      />
      <nav className="settings-tabs">
        {tabs.map((item) => (
          <button className={tab === item ? "active" : ""} onClick={() => setTab(item)} key={item}>
            {item}
          </button>
        ))}
      </nav>
      <section className="settings-stack">{content[tab as keyof typeof content]}</section>
    </main>
  );
}
