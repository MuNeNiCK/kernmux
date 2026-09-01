import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"
import "@xterm/xterm/css/xterm.css"
import { Show, createSignal, onCleanup, onMount } from "solid-js"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { CONSOLE_PROTOCOL, ConsoleFrame, ConsoleFrameDecoder, bearerSubprotocol, encodeFrame, type ConsoleAttachment } from "@/console-protocol"

export function InstanceConsole(props: { instanceId: number; name: string; token: string; onClose: () => void }) {
  let terminalElement: HTMLDivElement | undefined
  let socket: WebSocket | undefined
  let attached = false
  let readOutstanding = false
  let readTimer: number | undefined
  const [status, setStatus] = createSignal("Connecting")
  const [error, setError] = createSignal<string>()

  onMount(() => {
    if (!terminalElement) return
    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
      fontSize: 14,
      scrollback: 5000,
      theme: { background: "#0f1113", foreground: "#eef1f3", cursor: "#62c6a5", selectionBackground: "#35534c" },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(terminalElement)
    fit.fit()
    terminal.focus()

    const decoder = new ConsoleFrameDecoder()
    const scheme = window.location.protocol === "https:" ? "wss:" : "ws:"
    socket = new WebSocket(`${scheme}//${window.location.host}/api/1.0/instances/${props.instanceId}/console`, [CONSOLE_PROTOCOL, bearerSubprotocol(props.token)])
    socket.binaryType = "arraybuffer"

    const send = (kind: number, payload?: Uint8Array) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(encodeFrame(kind, payload))
    }
    const scheduleRead = (delay = 50) => {
      if (readOutstanding || readTimer !== undefined) return
      readTimer = window.setTimeout(() => {
        readTimer = undefined
        if (socket?.readyState === WebSocket.OPEN) {
          readOutstanding = true
          send(ConsoleFrame.read)
        }
      }, delay)
    }
    socket.onopen = () => setStatus("Negotiating")
    socket.onmessage = (event) => {
      if (!(event.data instanceof ArrayBuffer)) {
        setError("The gateway returned a non-binary console message.")
        socket?.close(1003, "binary frames required")
        return
      }
      try {
        for (const frame of decoder.push(event.data)) {
          if (frame.kind === ConsoleFrame.attachment) {
            const attachment = JSON.parse(new TextDecoder().decode(frame.payload)) as ConsoleAttachment
            if (attachment.instance_id !== props.instanceId || !attachment.capabilities.binary) throw new Error("The console attachment does not match this instance.")
            attached = true
            setStatus("Connected")
            scheduleRead(0)
          } else if (frame.kind === ConsoleFrame.output) {
            readOutstanding = false
            terminal.write(frame.payload)
            scheduleRead()
          } else if (frame.kind === ConsoleFrame.ack) {
            readOutstanding = false
            scheduleRead()
          } else if (frame.kind === ConsoleFrame.closed) {
            setStatus("Closed")
            terminal.writeln(`\r\n[console ${new TextDecoder().decode(frame.payload).replaceAll('"', "")}]`)
          } else if (frame.kind === ConsoleFrame.error) {
            const detail = JSON.parse(new TextDecoder().decode(frame.payload)) as { message?: string }
            throw new Error(detail.message ?? "The console rejected the request.")
          }
        }
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : "The console stream is invalid.")
        socket?.close(1002, "invalid console stream")
      }
    }
    socket.onerror = () => setError("Unable to connect to the instance console.")
    socket.onclose = () => setStatus("Disconnected")

    const input = terminal.onData((data) => send(ConsoleFrame.input, new TextEncoder().encode(data)))
    const resize = new ResizeObserver(() => fit.fit())
    resize.observe(terminalElement)
    onCleanup(() => {
      input.dispose()
      resize.disconnect()
      if (readTimer !== undefined) window.clearTimeout(readTimer)
      if (attached) send(ConsoleFrame.detach)
      socket?.close(1000, "console closed")
      terminal.dispose()
    })
  })

  return <Dialog open onOpenChange={(open) => { if (!open) props.onClose() }}><DialogContent class="grid-rows-[auto_1fr] overflow-hidden overscroll-contain p-0" style={{ width: "min(94vw, 1200px)", "max-width": "min(94vw, 1200px)", height: "min(80svh, 760px)", gap: "0" }} showCloseButton><DialogHeader class="min-w-0 border-b px-5 py-3"><div class="flex min-w-0 items-center gap-3 pr-10"><DialogTitle class="truncate">{props.name} console</DialogTitle><Badge variant="secondary" aria-live="polite">{status()}</Badge></div><DialogDescription class="text-xs">Direct MKTTY session for Instance {props.instanceId}. Only one client can be attached at a time.</DialogDescription></DialogHeader><div class="relative min-h-0 overscroll-contain bg-[#0f1113] p-2"><Show when={error()}>{(message) => <Alert role="alert" variant="destructive" class="absolute inset-x-2 top-2 z-10 bg-card"><AlertTitle>Console unavailable</AlertTitle><AlertDescription>{message()}</AlertDescription></Alert>}</Show><div ref={terminalElement} class="size-full overflow-hidden" aria-label={`${props.name} terminal`} /></div></DialogContent></Dialog>
}
