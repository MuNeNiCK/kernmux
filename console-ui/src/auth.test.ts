import { describe, expect, it, vi } from "vitest"
import { consumeFragmentToken } from "./auth"

describe("consumeFragmentToken", () => {
  it("returns a valid token and scrubs the fragment", () => {
    const replaceState = vi.fn()
    const token = "a".repeat(48)
    expect(consumeFragmentToken({ hash: `#token=${token}`, pathname: "/", search: "" } as Location, { replaceState } as unknown as History)).toBe(token)
    expect(replaceState).toHaveBeenCalledWith(null, "", "/")
  })
  it("rejects malformed credentials after scrubbing", () => {
    const replaceState = vi.fn()
    expect(() => consumeFragmentToken({ hash: "#token=short", pathname: "/", search: "" } as Location, { replaceState } as unknown as History)).toThrow(/valid management credential/)
    expect(replaceState).toHaveBeenCalled()
  })
})
