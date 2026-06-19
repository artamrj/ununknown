import type { Workflow } from "@/api";

export const processingPhases: Workflow["phase"][] = ["scan", "fetch", "apply"];

export const isProcessingPhase = (phase: Workflow["phase"]) => processingPhases.includes(phase);
