import { useQuery } from "@tanstack/react-query";
import { busyPhases } from "@/app/trackUtils";
import { api } from "./client";
import type { Setup, TrackPage, Workflow } from "./types";

export const queryKeys = {
  setup: ["setup"] as const,
  status: ["status"] as const,
  tracks: ["tracks"] as const,
};

export function useSetupQuery() {
  return useQuery({
    queryKey: queryKeys.setup,
    queryFn: () => api<Setup>("/setup"),
    refetchOnWindowFocus: false,
  });
}

export function useStatusQuery() {
  return useQuery({
    queryKey: queryKeys.status,
    queryFn: () => api<Workflow>("/status"),
    refetchInterval: (query) => {
      const phase = query.state.data?.phase;
      return phase && busyPhases.has(phase) ? 900 : false;
    },
    refetchIntervalInBackground: true,
    refetchOnWindowFocus: false,
  });
}

export function useTracksQuery(busy: boolean) {
  return useQuery({
    queryKey: queryKeys.tracks,
    queryFn: () => api<TrackPage>("/tracks").then((page) => page.items),
    refetchInterval: busy ? 5400 : false,
    refetchIntervalInBackground: true,
    refetchOnWindowFocus: false,
  });
}
