import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, Preview, Workflow } from "../api";
import { useEvents } from "../hooks";
import { Button, Toast } from "../components";
import { Flow, Workspace } from "../layouts";
import { SettingsPage } from "../pages/SettingsPage";

export function App() {
  const eventStatus = useEvents();
  const queryClient = useQueryClient();
  const [settingsPage, setSettingsPage] = useState(false);
  const [preview, setPreview] = useState<Preview>();
  const [toast, setToast] = useState("");
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api<any>("/settings") });
  const workflow = useQuery({
    queryKey: ["workspace"],
    queryFn: () => api<Workflow>("/workspace"),
    refetchInterval: 1500,
  });
  const scan = useMutation({
    mutationFn: () => api<any>("/scan/start", { method: "POST", body: "{}" }),
    onSuccess: () => queryClient.invalidateQueries(),
    onError: (error) => setToast(error.message),
  });
  const stop = useMutation({
    mutationFn: () => api<any>("/scan/stop", { method: "POST", body: "{}" }),
    onSuccess: () => queryClient.invalidateQueries(),
    onError: (error) => setToast(error.message),
  });
  const makePreview = useMutation({
    mutationFn: () => api<Preview>("/apply/preview", { method: "POST", body: "{}" }),
    onSuccess: setPreview,
    onError: (error) => setToast(error.message),
  });
  const apply = useMutation({
    mutationFn: () =>
      api("/apply/start", {
        method: "POST",
        body: JSON.stringify({ preview_token: preview?.preview_token }),
      }),
    onSuccess: () => {
      setPreview(undefined);
      queryClient.invalidateQueries();
    },
    onError: (error) => setToast(error.message),
  });

  useEffect(() => {
    if (workflow.data?.phase === "preview" && !preview && workflow.data.matched > 0) {
      makePreview.mutate();
    }
  }, [workflow.data?.phase, workflow.data?.matched, preview, makePreview]);

  if (settingsPage) {
    return <SettingsPage settings={settings.data} back={() => setSettingsPage(false)} />;
  }

  const active = !!workflow.data && ["scan", "fetch", "apply"].includes(workflow.data.phase);
  return (
    <>
      <header className="topbar">
        <b>
          <i>U</i> Ununknown <small>0.4.5</small>
        </b>
        <span>{settings.data?.input_dir}</span>
        <Button kind="quiet" onClick={() => setSettingsPage(true)}>
          Settings
        </Button>
      </header>
      <main className="pipeline v4">
        <Flow phase={workflow.data?.phase || "idle"} />
        <Workspace
          workflow={workflow.data}
          loading={workflow.isLoading}
          preview={preview}
          eventStatus={eventStatus}
          applyPending={apply.isPending}
          onScan={() => scan.mutate()}
          onStop={() => stop.mutate()}
          onApply={() =>
            confirm("Apply these metadata changes? Duplicate skips will not be written.") &&
            apply.mutate()
          }
        />
        {active && <div className="sr-only">Processing</div>}
      </main>
      <Toast text={toast} />
    </>
  );
}
