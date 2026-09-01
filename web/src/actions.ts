import type { InstanceState } from "./model"

export type LifecycleAction = "start" | "stop" | "unload" | "delete"

export function availableActions(state: InstanceState): LifecycleAction[] {
  if (state === "ready") return ["delete"]
  if (state === "loaded") return ["start", "unload"]
  if (state === "active") return ["stop"]
  return []
}
