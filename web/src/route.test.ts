import { describe, expect, it } from "vitest"
import { parseRoute, routeHref } from "./route"

describe("routes", () => {
  it("parses host and instance object views", () => {
    expect(parseRoute("#/host/manage")).toEqual({ kind: "host", tab: "manage" })
    expect(parseRoute("#/instances/2/monitor")).toEqual({ kind: "instance", id: 2, tab: "monitor" })
  })
  it("normalizes unknown routes to the host summary", () => {
    expect(routeHref(parseRoute("#/unknown"))).toBe("#/host/summary")
  })
})
