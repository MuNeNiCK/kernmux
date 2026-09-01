import { describe, expect, it, vi } from "vitest"
import { consumeTokenHash } from "./auth"

describe("consumeTokenHash", () => {
  it("consumes and removes a bearer token", () => {
    const replace = vi.fn()
    expect(consumeTokenHash("#token=secret%2Bvalue", replace)).toBe("secret+value")
    expect(replace).toHaveBeenCalledWith("")
  })
  it("does not reinterpret routes as credentials", () => {
    expect(consumeTokenHash("#/host/summary", vi.fn())).toBeNull()
  })
  it("preserves an explicit deep route while removing the token", () => {
    const replace = vi.fn()
    expect(consumeTokenHash("#token=secret&route=instances%2F2%2Fmanage", replace)).toBe("secret")
    expect(replace).toHaveBeenCalledWith("#/instances/2/manage")
  })
})
