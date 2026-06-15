import React, { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import "./index.css";

const api = async <T,>(path: string, init?: RequestInit): Promise<T> => {
  const r = await fetch(`/api${path}`, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  if (!r.ok) throw new Error((await r.json()).error || r.statusText);
  return r.json();
};
type Track = {
  id: number;
  path: string;
  filename: string;
  format?: string;
  current_title?: string;
  current_artist?: string;
  current_album?: string;
  selected_candidate_id?: number;
  status: string;
  error?: string;
};
type Candidate = {
  id: number;
  title?: string;
  artist?: string;
  album?: string;
  score: number;
};
const qc = new QueryClient();

function App() {
  const [page, setPage] = useState(location.hash.slice(1) || "dashboard");
  const query = useQueryClient();
  useEffect(() => {
    const es = new EventSource("/api/events");
    es.onmessage = () => query.invalidateQueries();
    return () => es.close();
  }, [query]);
  return (
    <div className="min-h-screen md:flex">
      <aside className="bg-slate-950 p-5 text-white md:w-60">
        <h1 className="mb-8 text-2xl font-black">Ununknown</h1>
        {["dashboard", "tracks", "review", "settings"].map((x) => (
          <button
            key={x}
            onClick={() => {
              location.hash = x;
              setPage(x);
            }}
            className={`mb-2 block w-full rounded-xl px-3 py-2 text-left capitalize ${page === x ? "bg-accent" : "hover:bg-slate-800"}`}
          >
            {x}
          </button>
        ))}
      </aside>
      <main className="max-w-7xl flex-1 p-5 md:p-8">
        {page === "dashboard" ? (
          <Dashboard />
        ) : page === "tracks" ? (
          <Tracks />
        ) : page === "review" ? (
          <Tracks review />
        ) : (
          <Settings />
        )}
      </main>
    </div>
  );
}
function Dashboard() {
  const query = useQueryClient();
  const { data: tracks = [] } = useQuery({
    queryKey: ["tracks"],
    queryFn: () => api<Track[]>("/tracks"),
  });
  const { data: jobs = [] } = useQuery({
    queryKey: ["jobs"],
    queryFn: () => api<any[]>("/jobs"),
  });
  const scan = useMutation({
    mutationFn: () => api("/scan/start", { method: "POST" }),
    onSuccess: () => query.invalidateQueries(),
  });
  const preview = useMutation({
    mutationFn: () =>
      api<any>("/apply/preview", { method: "POST", body: "{}" }),
  });
  const apply = useMutation({
    mutationFn: (token: string) =>
      api("/apply/start", {
        method: "POST",
        body: JSON.stringify({ preview_token: token }),
      }),
    onSuccess: () => {
      preview.reset();
      query.invalidateQueries();
    },
  });
  const counts = useMemo(
    () =>
      tracks.reduce<Record<string, number>>(
        (all, t) => ({ ...all, [t.status]: (all[t.status] || 0) + 1 }),
        {},
      ),
    [tracks],
  );
  const job = jobs[0];
  return (
    <>
      <Header
        title="Dashboard"
        subtitle="Scan, review, preview, then apply metadata safely."
      />
      <div className="grid gap-4 sm:grid-cols-4">
        {[
          ["Files", tracks.length],
          ["Ready", counts.selected || 0],
          ["Review", counts.needs_review || 0],
          ["Applied", counts.applied || 0],
        ].map(([a, b]) => (
          <div className="card" key={a}>
            <div className="text-sm text-muted">{a}</div>
            <div className="mt-2 text-3xl font-black">{b}</div>
          </div>
        ))}
      </div>
      <div className="card mt-5">
        <div className="flex flex-wrap gap-3">
          <button className="primary" onClick={() => scan.mutate()}>
            Scan library
          </button>
          <button className="secondary" onClick={() => preview.mutate()}>
            Dry-run preview
          </button>
          {preview.data && (
            <button
              className="primary"
              onClick={() =>
                confirm(`Apply ${preview.data.items.length} changes?`) &&
                apply.mutate(preview.data.preview_token)
              }
            >
              Apply {preview.data.items.length} changes
            </button>
          )}
        </div>
        {job && (
          <div className="mt-5">
            <div className="mb-2 flex justify-between text-sm">
              <span>
                {job.kind}: {job.status}
              </span>
              <span>
                {job.progress_current}/{job.progress_total}
              </span>
            </div>
            <div className="h-2 overflow-hidden rounded bg-slate-200">
              <div
                className="h-full bg-accent"
                style={{
                  width: `${job.progress_total ? (job.progress_current / job.progress_total) * 100 : 0}%`,
                }}
              />
            </div>
          </div>
        )}
      </div>
      {preview.data && <Preview items={preview.data.items} />}
    </>
  );
}
function Preview({ items }: { items: any[] }) {
  return (
    <div className="card mt-5">
      <h2 className="mb-4 text-xl font-bold">Required dry-run preview</h2>
      <div className="space-y-3">
        {items.map((x) => (
          <div className="rounded-xl bg-slate-50 p-3" key={x.track_id}>
            <div className="text-sm text-muted">{x.current_path}</div>
            <div className="font-semibold">→ {x.destination_path}</div>
            <div className="mt-1 text-xs">
              {x.action}{" "}
              {x.warnings.map((w: string) => (
                <span className="text-amber-700" key={w}>
                  {" "}
                  · {w}
                </span>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
function Tracks({ review = false }: { review?: boolean }) {
  const query = useQueryClient();
  const { data = [] } = useQuery({
    queryKey: ["tracks"],
    queryFn: () => api<Track[]>("/tracks"),
  });
  const [search, setSearch] = useState("");
  const tracks = data.filter(
    (t) =>
      (!review || t.status === "needs_review") &&
      (t.path.toLowerCase().includes(search.toLowerCase()) ||
        t.current_title?.toLowerCase().includes(search.toLowerCase())),
  );
  return (
    <>
      <Header
        title={review ? "Review" : "Tracks"}
        subtitle={
          review
            ? "Choose candidates for low-confidence matches."
            : "Current library workspace."
        }
      />
      <input
        className="input mb-4 max-w-md"
        placeholder="Search tracks"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <div className="card overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead>
            <tr className="border-b">
              <th className="p-2">File</th>
              <th>Current</th>
              <th>Status</th>
              <th>Candidate</th>
            </tr>
          </thead>
          <tbody>
            {tracks.map((t) => (
              <TrackRow
                key={t.id}
                track={t}
                done={() => query.invalidateQueries()}
              />
            ))}
          </tbody>
        </table>
        {!tracks.length && <Empty />}
      </div>
    </>
  );
}
function TrackRow({ track, done }: { track: Track; done: () => void }) {
  const { data = [] } = useQuery({
    queryKey: ["candidates", track.id],
    queryFn: () => api<Candidate[]>(`/tracks/${track.id}/candidates`),
  });
  const select = useMutation({
    mutationFn: (candidate_id: number | null) =>
      api(`/tracks/${track.id}/select-candidate`, {
        method: "POST",
        body: JSON.stringify({ candidate_id }),
      }),
    onSuccess: done,
  });
  return (
    <tr className="border-b last:border-0">
      <td className="p-2">
        <div className="font-semibold">{track.filename}</div>
        <div className="max-w-xs truncate text-xs text-muted">{track.path}</div>
      </td>
      <td>
        {track.current_artist || "Unknown"} · {track.current_title || "Unknown"}
      </td>
      <td>
        <span className="badge">{track.status}</span>
      </td>
      <td>
        <select
          className="input min-w-52"
          value={track.selected_candidate_id || ""}
          onChange={(e) =>
            select.mutate(e.target.value ? Number(e.target.value) : null)
          }
        >
          <option value="">Skip / choose…</option>
          {data.map((c) => (
            <option key={c.id} value={c.id}>
              {Math.round(c.score)}% · {c.artist} — {c.title}
            </option>
          ))}
        </select>
      </td>
    </tr>
  );
}
function Settings() {
  const query = useQueryClient();
  const { data } = useQuery({
    queryKey: ["settings"],
    queryFn: () => api<any>("/settings"),
  });
  const [form, setForm] = useState<any>();
  useEffect(() => {
    if (data) setForm(data);
  }, [data]);
  const save = useMutation({
    mutationFn: () =>
      api("/settings", { method: "PUT", body: JSON.stringify(form) }),
    onSuccess: () => query.invalidateQueries(),
  });
  if (!form) return <Empty />;
  return (
    <>
      <Header
        title="Settings"
        subtitle="Provider secrets are configured only in TOML or environment variables."
      />
      <div className="grid gap-5 lg:grid-cols-2">
        <section className="card space-y-4">
          <h2 className="text-xl font-bold">Folders and behavior</h2>
          <Field
            label="Input folder"
            value={form.input_dir}
            set={(v) => setForm({ ...form, input_dir: v })}
          />
          <Field
            label="Output folder"
            value={form.output_dir}
            set={(v) => setForm({ ...form, output_dir: v })}
          />
          <label>
            Output mode
            <select
              className="input mt-1"
              value={form.output_mode}
              onChange={(e) =>
                setForm({ ...form, output_mode: e.target.value })
              }
            >
              <option value="copy">Copy to output</option>
              <option value="in_place">In-place</option>
            </select>
          </label>
          <Field
            label="Path template"
            value={form.path_templates.default_template}
            set={(v) =>
              setForm({
                ...form,
                path_templates: { ...form.path_templates, default_template: v },
              })
            }
          />
          <label className="flex gap-2">
            <input
              type="checkbox"
              checked={form.overwrite_existing_tags}
              onChange={(e) =>
                setForm({ ...form, overwrite_existing_tags: e.target.checked })
              }
            />
            Overwrite existing tags
          </label>
          <button className="primary" onClick={() => save.mutate()}>
            Save settings
          </button>
        </section>
        <section className="card">
          <h2 className="text-xl font-bold">Providers</h2>
          <p className="mt-3">
            AcoustID:{" "}
            <span className="badge">
              {form.acoustid_configured ? "configured" : "not configured"}
            </span>
          </p>
          <p className="mt-4 text-sm text-muted">
            Set <code>acoustid_api_key</code> and a meaningful{" "}
            <code>musicbrainz_user_agent</code> in{" "}
            <code>/config/config.toml</code>, then restart the container.
            Secrets are never returned by this API.
          </p>
          <h2 className="mt-8 text-xl font-bold">In-place safety</h2>
          {[
            ["Rename files", "rename_files"],
            ["Reorganize folders", "rename_folders"],
          ].map(([label, key]) => (
            <label className="mt-3 flex gap-2" key={key}>
              <input
                type="checkbox"
                checked={form.in_place[key]}
                onChange={(e) =>
                  setForm({
                    ...form,
                    in_place: { ...form.in_place, [key]: e.target.checked },
                  })
                }
              />
              {label}
            </label>
          ))}
          <p className="mt-3 text-sm text-amber-700">
            These change your existing structure. Test on copied files first.
          </p>
        </section>
      </div>
    </>
  );
}
function Field({
  label,
  value,
  set,
}: {
  label: string;
  value: string;
  set: (v: string) => void;
}) {
  return (
    <label className="block">
      {label}
      <input
        className="input mt-1"
        value={value}
        onChange={(e) => set(e.target.value)}
      />
    </label>
  );
}
function Header({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <header className="mb-6">
      <h1 className="text-3xl font-black">{title}</h1>
      <p className="text-muted">{subtitle}</p>
    </header>
  );
}
function Empty() {
  return <div className="p-8 text-center text-muted">Nothing to show yet.</div>;
}
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={qc}>
    <App />
  </QueryClientProvider>,
);
