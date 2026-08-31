export interface Cpu { logical_id: number; hardware_id: number; package_id: number; core_id: number; thread_index: number; numa_node: number; online: boolean }
export interface NumaNode { id: number; logical_cpu_ids: number[]; total_memory_bytes: number; available_memory_bytes: number }
export interface Diagnostic { code?: string; severity: string; message: string; detail?: string; redacted?: boolean }
export interface Instance { id: number; name: string; generation: number; state: string; resources: { cpu_hardware_ids: number[]; memory_base?: number; memory_bytes: number; memory_region?: string; device_ids: string[] }; image: { present: boolean } }
export interface ImageArtifact { schema_version: number; kind: string; id: string; bytes: number }
export interface ResourceReference { kind: string; id: string }
export interface Operation { id: string; kind: string; state: string; progress_percent?: number; expected_generation: number; observed_generation?: number; affected_resources?: ResourceReference[]; actor?: { uid: number; gid: number; label?: string }; audit_id?: string; created_at: string; completed_at?: string; error?: { message: string; diagnostics?: Diagnostic[] } }
export interface Transaction { id: string; state: string; generation_before?: number; generation_after?: number; diagnostics?: Diagnostic[] }
export interface Event { sequence: number; snapshot_generation: number; kind: string; resource?: ResourceReference }
export interface EventPage { events: Event[]; overflowed: boolean; latest_sequence: number }
export interface HostSnapshot {
  generation: number; health: string; diagnostics?: Diagnostic[];
  kernel: { release: string; multikernel_enabled: boolean }; capabilities: string[];
  topology: { architecture: string; cpus: Cpu[]; numa_nodes: NumaNode[] };
  memory: { total_bytes: number; host_reserved_bytes: number; assignable_bytes: number; assigned_bytes: number };
  resource_pool: { cpu_hardware_ids: number[]; available_cpu_hardware_ids: number[]; memory_regions?: Array<{ base: number; bytes: number; numa_node: number }>; devices: Array<{ pci_id: string; pool_name: string; vendor_id?: number; device_id?: number; iommu_group?: number; iommu_group_members?: string[] }>; available_device_ids: string[] };
  instances: Instance[]; transactions: Transaction[]; operations: Operation[];
}

export function normalizeHostSnapshot(snapshot: HostSnapshot): HostSnapshot {
  const instances = (snapshot.instances ?? []).map(instance => ({
    ...instance,
    resources: {
      ...instance.resources,
      cpu_hardware_ids: instance.resources.cpu_hardware_ids ?? [],
      device_ids: instance.resources.device_ids ?? [],
    },
  }))
  const assignedCpus = new Set(instances.flatMap(instance => instance.resources.cpu_hardware_ids))
  const assignedDevices = new Set(instances.flatMap(instance => instance.resources.device_ids))
  const devices = snapshot.resource_pool.devices ?? []
  return {
    ...snapshot,
    diagnostics: snapshot.diagnostics ?? [],
    instances,
    transactions: snapshot.transactions ?? [],
    operations: snapshot.operations ?? [],
    resource_pool: {
      ...snapshot.resource_pool,
      cpu_hardware_ids: snapshot.resource_pool.cpu_hardware_ids ?? [],
      available_cpu_hardware_ids: snapshot.resource_pool.available_cpu_hardware_ids
        ?? (snapshot.resource_pool.cpu_hardware_ids ?? []).filter(id => !assignedCpus.has(id)),
      memory_regions: snapshot.resource_pool.memory_regions ?? [],
      devices,
      available_device_ids: snapshot.resource_pool.available_device_ids
        ?? devices.map(device => device.pci_id).filter(id => !assignedDevices.has(id)),
    },
  }
}
type ResultEnvelope<T> = { kind: "result"; generation: number; data: T }
type AcceptedEnvelope = { kind: "accepted"; operation: Operation }
type ErrorEnvelope = { kind: "error"; error: { message: string } }
type Envelope<T> = ResultEnvelope<T> | AcceptedEnvelope | ErrorEnvelope

export class ApiClient {
  constructor(private readonly token: string) {}
  async request<T>(method: string, path: string, body?: unknown): Promise<Envelope<T>> {
    const response = await fetch(path, { method, headers: {
      Authorization: `Bearer ${this.token}`, Accept: "application/json",
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    }, body: body === undefined ? undefined : JSON.stringify(body) })
    const envelope = await response.json() as Envelope<T>
    if (!response.ok || envelope.kind === "error") {
      throw new Error(envelope.kind === "error" ? envelope.error.message : `Management request failed (${response.status}).`)
    }
    return envelope
  }
  async result<T>(path: string): Promise<T> {
    const envelope = await this.request<T>("GET", path)
    if (envelope.kind !== "result") throw new Error("Management response did not contain a result.")
    return envelope.data
  }
  async mutate<T>(method: string, path: string, body?: unknown): Promise<void> {
    const envelope = await this.request<T>(method, path, body)
    if (envelope.kind !== "accepted") return
    let operation = envelope.operation
    for (let attempt = 0; attempt < 120 && ["queued", "running"].includes(operation.state); attempt += 1) {
      await new Promise(resolve => window.setTimeout(resolve, 500))
      operation = await this.result<Operation>(`/api/1.0/operations/${encodeURIComponent(operation.id)}`)
    }
    if (operation.state !== "succeeded") throw new Error(operation.error?.message ?? `Operation ${operation.state}.`)
  }
  async cancelOperation(id: string): Promise<void> {
    const envelope = await this.request<Operation>("DELETE", `/api/1.0/operations/${encodeURIComponent(id)}`)
    if (envelope.kind !== "accepted") throw new Error("Cancellation was not accepted.")
  }
}
