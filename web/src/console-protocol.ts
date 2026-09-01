export const CONSOLE_PROTOCOL = "kernmux-console-v1"
export const MAX_CONSOLE_PAYLOAD = 64 * 1024

export const ConsoleFrame = {
  attachment: 0x01,
  input: 0x10,
  read: 0x11,
  detach: 0x12,
  output: 0x20,
  ack: 0x21,
  closed: 0x22,
  error: 0x7f,
} as const

export interface ConsoleAttachment {
  instance_id: number
  capabilities: { binary: boolean; resize: boolean }
  max_frame_bytes: number
}
export interface DecodedFrame {
  kind: number
  payload: Uint8Array
}

export function bearerSubprotocol(token: string): string {
  const bytes = new TextEncoder().encode(token)
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return `kernmux-bearer.${btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")}`
}

export function encodeFrame(kind: number, payload: Uint8Array = new Uint8Array()): ArrayBuffer {
  if (payload.byteLength > MAX_CONSOLE_PAYLOAD) throw new Error("Console frame exceeds 64 KiB.")
  const frame = new Uint8Array(5 + payload.byteLength)
  frame[0] = kind
  new DataView(frame.buffer).setUint32(1, payload.byteLength, false)
  frame.set(payload, 5)
  return frame.buffer
}

export class ConsoleFrameDecoder {
  private buffered = new Uint8Array()

  push(chunk: ArrayBuffer): DecodedFrame[] {
    const incoming = new Uint8Array(chunk)
    if (this.buffered.byteLength + incoming.byteLength > MAX_CONSOLE_PAYLOAD * 2 + 10) {
      throw new Error("Console stream buffer exceeded its limit.")
    }
    const combined = new Uint8Array(this.buffered.byteLength + incoming.byteLength)
    combined.set(this.buffered)
    combined.set(incoming, this.buffered.byteLength)
    const frames: DecodedFrame[] = []
    let offset = 0
    while (combined.byteLength - offset >= 5) {
      const length = new DataView(combined.buffer, combined.byteOffset + offset + 1, 4).getUint32(0, false)
      if (length > MAX_CONSOLE_PAYLOAD) throw new Error("Console frame exceeds 64 KiB.")
      if (combined.byteLength - offset < 5 + length) break
      frames.push({ kind: combined[offset], payload: combined.slice(offset + 5, offset + 5 + length) })
      offset += 5 + length
    }
    this.buffered = combined.slice(offset)
    return frames
  }
}
