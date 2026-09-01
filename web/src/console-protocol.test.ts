import { describe, expect, it, vi } from "vitest"

import { ConsoleFrame, ConsoleFrameDecoder, bearerSubprotocol, encodeFrame } from "./console-protocol"

describe("console protocol", () => {
  it("encodes bearer credentials as a WebSocket-safe subprotocol", () => {
    vi.stubGlobal("btoa", (value: string) => Buffer.from(value, "binary").toString("base64"))
    expect(bearerSubprotocol("abc+/=")).toBe("kernmux-bearer.YWJjKy89")
    vi.unstubAllGlobals()
  })

  it("decodes fragmented and adjacent binary frames", () => {
    const first = new Uint8Array(encodeFrame(ConsoleFrame.output, new TextEncoder().encode("hi")))
    const second = new Uint8Array(encodeFrame(ConsoleFrame.ack))
    const decoder = new ConsoleFrameDecoder()
    expect(decoder.push(first.slice(0, 3).buffer)).toEqual([])
    const tail = new Uint8Array(first.byteLength - 3 + second.byteLength)
    tail.set(first.slice(3))
    tail.set(second, first.byteLength - 3)
    const frames = decoder.push(tail.buffer)
    expect(new TextDecoder().decode(frames[0].payload)).toBe("hi")
    expect(frames[1]).toEqual({ kind: ConsoleFrame.ack, payload: new Uint8Array() })
  })

  it("rejects oversized frames before allocation", () => {
    const header = new Uint8Array([ConsoleFrame.output, 0, 1, 0, 1])
    expect(() => new ConsoleFrameDecoder().push(header.buffer)).toThrow("exceeds 64 KiB")
  })
})
