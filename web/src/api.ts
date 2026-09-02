import type { AcceptedEnvelope, EventPage, HostSnapshot, ImageArtifact, Instance, Operation, OsImage, ResultEnvelope } from "./model"

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message)
  }
}

function errorMessage(value: unknown): { code: string; message: string } {
  if (typeof value === "object" && value !== null) {
    const record = value as Record<string, unknown>
    if (typeof record.error === "string") {
      return { code: record.error, message: record.error.replaceAll("_", " ") }
    }
    const nested = typeof record.error === "object" && record.error !== null
      ? record.error as Record<string, unknown>
      : record
    const code = typeof nested.code === "string" ? nested.code : "request_failed"
    const message = typeof nested.message === "string" ? nested.message : code.replaceAll("_", " ")
    return { code, message }
  }
  return { code: "request_failed", message: "The management request failed." }
}

export class KernmuxApi {
  constructor(private readonly bearer: string) {}

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`/api/1.0${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${this.bearer}`,
        ...(init?.body ? { "Content-Type": "application/json" } : {}),
        ...init?.headers,
      },
    })
    const value: unknown = await response.json().catch(() => null)
    if (!response.ok) {
      const detail = errorMessage(value)
      throw new ApiError(response.status, detail.code, detail.message)
    }
    return value as T
  }

  host(): Promise<ResultEnvelope<HostSnapshot>> { return this.request("") }
  instances(): Promise<ResultEnvelope<Instance[]>> { return this.request("/instances") }
  images(): Promise<ResultEnvelope<ImageArtifact[]>> { return this.request("/images") }
  osImages(): Promise<ResultEnvelope<OsImage[]>> { return this.request("/os-images") }
  operations(): Promise<ResultEnvelope<Operation[]>> { return this.request("/operations") }
  events(): Promise<ResultEnvelope<EventPage>> { return this.request("/events") }
  operation(id: string): Promise<ResultEnvelope<Operation>> { return this.request(`/operations/${encodeURIComponent(id)}`) }

  lifecycle(id: number, action: "start" | "stop" | "unload", generation: number) {
    return this.request<AcceptedEnvelope>(`/instances/${id}/${action}`, {
      method: "POST",
      body: JSON.stringify({ expected_generation: generation, ...(action === "stop" ? { force: false } : {}) }),
    })
  }

  createInstance(input: { expected_generation: number; id: number; name: string; cpu_hardware_ids: number[]; memory_bytes: number }) {
    return this.accepted("/instances", "POST", input)
  }

  updateInstance(id: number, input: { expected_generation: number; cpu_hardware_ids?: number[]; memory_bytes?: number; device_ids?: string[]; dry_run: boolean }) {
    return this.accepted(`/instances/${id}`, "PATCH", input)
  }

  loadManagedImage(id: number, input: { expected_generation: number; kernel_id: string; initrd_id?: string; command_line?: string }) {
    return this.accepted(`/instances/${id}/load-image`, "POST", input)
  }

  deleteInstance(id: number, generation: number) {
    return this.accepted(`/instances/${id}`, "DELETE", { expected_generation: generation })
  }

  importImage(input: { expected_generation: number; kind: "kernel" | "initrd"; source_path: string; expected_id?: string }) {
    return this.accepted("/images", "POST", input)
  }

  async uploadOsImage(input: { expected_generation: number; file: File; label: string; architecture?: string; expected_sha256?: string }): Promise<AcceptedEnvelope> {
    const body = new FormData()
    body.append("label", input.label)
    body.append("expected_generation", String(input.expected_generation))
    if (input.architecture) body.append("architecture", input.architecture)
    if (input.expected_sha256) body.append("expected_sha256", input.expected_sha256)
    body.append("file", input.file, input.file.name)
    const response = await fetch("/api/1.0/os-images/upload", {
      method: "POST",
      headers: { Authorization: `Bearer ${this.bearer}` },
      body,
    })
    const value: unknown = await response.json().catch(() => null)
    if (!response.ok) {
      const detail = errorMessage(value)
      throw new ApiError(response.status, detail.code, detail.message)
    }
    return value as AcceptedEnvelope
  }

  private accepted(path: string, method: string, body: object): Promise<AcceptedEnvelope> {
    return this.request<AcceptedEnvelope>(path, { method, body: JSON.stringify(body) })
  }
}
