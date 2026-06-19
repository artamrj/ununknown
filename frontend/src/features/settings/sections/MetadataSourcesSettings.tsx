import type { SettingsSectionProps } from "@/features/settings/types";
import { Choice, Reset, Section } from "@/features/settings/components/SettingsFields";

const modes = ["primary", "fallback", "parallel", "enrichment_only"];
const strategyHelp: Record<string, [string, string]> = {
  safe: ["Safe", "Primary sources first; uncertain files always go to review."],
  balanced: ["Balanced", "Uses fallback sources on medium confidence and compares results."],
  aggressive: ["Aggressive", "Queries sources in parallel and accepts more automatic matches."],
};

const providers = [
  {
    id: "musicbrainz",
    name: "MusicBrainz",
    badge: "MB",
    desc: "Canonical open music metadata.",
    key: "user_agent",
  },
  {
    id: "acoustid",
    name: "AcoustID",
    badge: "AID",
    desc: "Fingerprint-based recording candidates.",
    key: "api_key",
  },
  {
    id: "discogs",
    name: "Discogs",
    badge: "DG",
    desc: "Release, label, catalog, and physical media metadata.",
    key: "token",
  },
  {
    id: "cover_art_archive",
    name: "Cover Art Archive",
    badge: "CAA",
    desc: "Album cover enrichment from MusicBrainz releases.",
    key: false,
  },
  {
    id: "theaudiodb",
    name: "TheAudioDB",
    badge: "ADB",
    desc: "Artist and album enrichment, images, and fan metadata.",
    key: "api_key",
  },
  {
    id: "wikidata",
    name: "Wikidata",
    badge: "WD",
    desc: "External links, identifiers, and structured enrichment.",
    key: false,
  },
  {
    id: "lastfm",
    name: "Last.fm",
    badge: "FM",
    desc: "Tags, popularity signals, and artist enrichment.",
    key: "api_key",
  },
] as const;

export function MetadataSourcesSettings({
  settings,
  visible,
  set,
  reset,
}: SettingsSectionProps) {
  const updateProvider = (id: string, key: string, value: unknown) =>
    set("metadata_sources", {
      ...settings.metadata_sources,
      [id]: { ...settings.metadata_sources?.[id], [key]: value },
    });

  return (
    <>
      <Section
        title="Matching strategy"
        note="Controls when fallback and parallel metadata sources are used."
      >
        <Choice
          value={settings.matching_strategy || "balanced"}
          set={(value) => set("matching_strategy", value)}
          items={Object.entries(strategyHelp).map(([key, value]) => [key, value[0], value[1]])}
        />
      </Section>
      <Section
        title="Metadata Sources"
        note="Choose which providers participate in matching decisions or enrichment."
      >
        <div className="provider-grid">
          {providers
            .filter((provider) => visible(`${provider.name} ${provider.desc}`))
            .map((provider) => {
              const config = settings.metadata_sources?.[provider.id] || {};
              const status = settings.provider_statuses?.[provider.id];
              return (
                <article className="provider-card" key={provider.id}>
                  <header>
                    <span className="provider-badge">{provider.badge}</span>
                    <div>
                      <h3>{provider.name}</h3>
                      <p>{provider.desc}</p>
                    </div>
                    <label className="provider-toggle">
                      <input
                        checked={Boolean(config.enabled)}
                        onChange={(event) =>
                          updateProvider(provider.id, "enabled", event.target.checked)
                        }
                        type="checkbox"
                      />
                    </label>
                  </header>
                  <div className={`provider-status ${status?.status || "disabled"}`}>
                    {label(status?.status || "disabled")}
                  </div>
                  <label>
                    <span>Mode</span>
                    <select
                      value={config.mode || "enrichment_only"}
                      onChange={(event) => updateProvider(provider.id, "mode", event.target.value)}
                    >
                      {modes.map((mode) => (
                        <option key={mode} value={mode}>
                          {label(mode)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <small>Confidence weight: {status?.confidence_weight || "Enrichment"}</small>
                  {provider.key && (
                    <label>
                      <span>{credentialLabel(provider.key)}</span>
                      <input
                        value={config[provider.key] || ""}
                        placeholder={
                          provider.key === "user_agent"
                            ? "Ununknown/0.6.0 (you@example.com)"
                            : status?.configured
                              ? "configured"
                              : ""
                        }
                        onChange={(event) =>
                          updateProvider(provider.id, provider.key, event.target.value)
                        }
                      />
                    </label>
                  )}
                </article>
              );
            })}
        </div>
      </Section>
      <Reset onClick={() => reset("sources")} />
    </>
  );
}

function label(value: string) {
  return value.replaceAll("_", " ").replace(/\b\w/g, (char) => char.toUpperCase());
}

function credentialLabel(value: string) {
  if (value === "token") return "API token";
  if (value === "user_agent") return "User-Agent / contact";
  return "API key";
}
