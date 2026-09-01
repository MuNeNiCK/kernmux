export type InstanceState = "absent" | "ready" | "loaded" | "active" | "unknown"
export type OperationState = "queued" | "running" | "succeeded" | "failed" | "cancelled" | "indeterminate" | "unknown"

export interface Cpu {
  logical_id: number
  hardware_id: number
  package_id: number
  core_id: number
  thread_index: number
  numa_node: number
  online: boolean
}

export interface NumaNode {
  id: number
  logical_cpu_ids: number[]
  total_memory_bytes: number
  available_memory_bytes: number
}

export interface Instance {
  id: number
  name: string
  generation: number
  state: InstanceState
  image: { present: boolean }
  resources: {
    cpu_hardware_ids: number[]
    memory_base?: number
    memory_bytes: number
    memory_region?: string
    device_ids?: string[]
  }
}

export interface Diagnostic {
  code: string
  severity: "info" | "warning" | "error" | "unknown"
  message: string
  detail?: string
}

export interface Operation {
  id: string
  kind: string
  state: OperationState
  progress_percent?: number
  expected_generation: number
  observed_generation?: number
  affected_resources?: Array<{ kind: string; id: string }>
  error?: { code?: string; message?: string }
  created_at: string
  completed_at?: string
}

export interface Transaction {
  id: string
  state: "planned" | "applied" | "rolled_back" | "failed" | "unknown"
  generation_before?: number
  generation_after?: number
  diagnostics?: Diagnostic[]
}

export interface ImageArtifact {
  schema_version: number
  kind: "kernel" | "initrd" | "unknown"
  id: string
  bytes: number
}

export interface HostSnapshot {
  generation: number
  health: "healthy" | "indeterminate" | "unknown"
  diagnostics?: Diagnostic[]
  kernel: { release: string; multikernel_enabled: boolean }
  capabilities: string[]
  topology: { architecture: string; cpus: Cpu[]; numa_nodes: NumaNode[] }
  memory: {
    total_bytes: number
    host_reserved_bytes: number
    assignable_bytes: number
    assigned_bytes: number
  }
  resource_pool: {
    cpu_hardware_ids: number[]
    available_cpu_hardware_ids?: number[]
    memory_regions: Array<{ base: number; bytes: number; numa_node: number }>
    devices?: Array<{ pci_id: string; pool_name: string; iommu_group?: number }>
    available_device_ids?: string[]
  }
  instances: Instance[]
  operations?: Operation[]
  transactions?: Transaction[]
}

export interface EventPage {
  events: Array<{
    sequence: number
    snapshot_generation: number
    kind: string
    resource?: { kind: string; id: string }
  }>
  latest_sequence: number
  overflowed: boolean
}

export interface ResultEnvelope<T> {
  kind: "result"
  generation: number
  data: T
}

export interface AcceptedEnvelope {
  kind: "accepted"
  operation: Operation
}
