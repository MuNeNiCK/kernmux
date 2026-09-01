import { describe, expect, it } from "vitest"
import { availableActions } from "./actions"

describe("availableActions", () => {
  it("matches authoritative lifecycle states", () => {
    expect(availableActions("ready")).toEqual(["delete"])
    expect(availableActions("loaded")).toEqual(["start", "unload"])
    expect(availableActions("active")).toEqual(["stop"])
    expect(availableActions("unknown")).toEqual([])
  })
})
