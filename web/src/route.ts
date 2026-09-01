export type ViewTab = "summary" | "monitor" | "manage"
export type Route =
  | { kind: "host"; tab: ViewTab }
  | { kind: "instances" }
  | { kind: "instance"; id: number; tab: ViewTab }
  | { kind: "images" }

const tabs = new Set<ViewTab>(["summary", "monitor", "manage"])

export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#\/?/, "").split("?")[0]
  const parts = path.split("/").filter(Boolean)
  if (parts[0] === "host") {
    const tab = parts[1] as ViewTab
    return { kind: "host", tab: tabs.has(tab) ? tab : "summary" }
  }
  if (parts[0] === "instances" && parts.length === 1) return { kind: "instances" }
  if (parts[0] === "instances" && /^\d+$/.test(parts[1] ?? "")) {
    const tab = parts[2] as ViewTab
    return { kind: "instance", id: Number(parts[1]), tab: tabs.has(tab) ? tab : "summary" }
  }
  if (parts[0] === "images") return { kind: "images" }
  return { kind: "host", tab: "summary" }
}

export function routeHref(route: Route): string {
  if (route.kind === "host") return `#/host/${route.tab}`
  if (route.kind === "instances") return "#/instances"
  if (route.kind === "images") return "#/images"
  return `#/instances/${route.id}/${route.tab}`
}
