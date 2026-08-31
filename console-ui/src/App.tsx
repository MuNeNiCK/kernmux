import { For, Match, Show, Switch, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js"
import ActivityIcon from "lucide-solid/icons/activity"
import BoxIcon from "lucide-solid/icons/box"
import CpuIcon from "lucide-solid/icons/cpu"
import HardDriveIcon from "lucide-solid/icons/hard-drive"
import ImageIcon from "lucide-solid/icons/image"
import Layers3Icon from "lucide-solid/icons/layers-3"
import PlusIcon from "lucide-solid/icons/plus"
import RefreshCwIcon from "lucide-solid/icons/refresh-cw"
import ServerIcon from "lucide-solid/icons/server"
import Trash2Icon from "lucide-solid/icons/trash-2"
import TriangleAlertIcon from "lucide-solid/icons/triangle-alert"

import { ApiClient, normalizeHostSnapshot, type Event, type HostSnapshot, type ImageArtifact, type Instance, type Operation } from "./api"
import { consumeFragmentToken } from "./auth"
import { ConsoleShell } from "./components/console-shell"
import DataTableView, { type ConsoleColumn, type ConsoleRow } from "./components/data-table-view"
import DetailView, { DetailList, DetailSection } from "./components/detail-view"
import DoubleConfirmation from "./components/double-confirmation-dialog"
import EmptyState from "./components/empty-state"
import FormFlow from "./components/form-flow"
import { Alert, AlertDescription, AlertTitle } from "./components/ui/alert"
import { Badge } from "./components/ui/badge"
import { Button } from "./components/ui/button"
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "./components/ui/field"
import { Input } from "./components/ui/input"
import { NativeSelect, NativeSelectOption } from "./components/ui/native-select"
import { Progress } from "./components/ui/progress"
import { Resizable, ResizableHandle, ResizablePanel } from "./components/ui/resizable"
import { ScrollArea } from "./components/ui/scroll-area"
import { SidebarGroup, SidebarGroupContent, SidebarGroupLabel, SidebarMenu, SidebarMenuBadge, SidebarMenuButton, SidebarMenuItem } from "./components/ui/sidebar"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "./components/ui/table"
import { Tabs, TabsList, TabsTrigger } from "./components/ui/tabs"

type Selection = { kind: "host" } | { kind: "instance"; id: number } | { kind: "images" } | { kind: "operations" }
type Tab = "summary" | "monitor" | "manage"
const number = new Intl.NumberFormat()
const bytes = new Intl.NumberFormat(undefined, { style: "unit", unit: "gigabyte", unitDisplay: "short", maximumFractionDigits: 1 })
const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" })
const formatBytes = (value: number) => bytes.format(value / 2 ** 30)
const titleCase = (value: string) => value.replaceAll("_", " ").replace(/\b\w/g, letter => letter.toUpperCase())
const statusVariant = (state: string) => ["healthy", "active", "succeeded", "applied", "loaded", "ready"].includes(state) ? "default" : ["failed", "indeterminate"].includes(state) ? "destructive" : "outline"

function initialNavigation(): { selection: Selection; tab: Tab } {
  const params = new URLSearchParams(location.search)
  const requested = params.get("tab")
  const tab: Tab = requested === "monitor" || requested === "manage" ? requested : "summary"
  const object = params.get("object") ?? "host"
  if (object === "images" || object === "operations") return { selection: { kind: object }, tab }
  if (object.startsWith("instance:")) { const id = Number(object.slice(9)); if (Number.isInteger(id) && id >= 0) return { selection: { kind: "instance", id }, tab } }
  return { selection: { kind: "host" }, tab }
}

function parseCpuList(value: string) {
  const cpus = value.split(",").flatMap(part => { const [start, end] = part.trim().split("-").map(Number); if (!Number.isInteger(start)) return []; if (end === undefined) return [start]; if (!Number.isInteger(end) || start > end) return []; return Array.from({ length: end - start + 1 }, (_, index) => start + index) })
  return cpus.length && new Set(cpus).size === cpus.length ? cpus : []
}

export function App() {
  const initial = initialNavigation()
  const [client, setClient] = createSignal<ApiClient>()
  const [host, setHost] = createSignal<HostSnapshot>()
  const [images, setImages] = createSignal<ImageArtifact[]>([])
  const [events, setEvents] = createSignal<Event[]>([])
  const [selection, setSelection] = createSignal<Selection>(initial.selection)
  const [tab, setTab] = createSignal<Tab>(initial.tab)
  const [loading, setLoading] = createSignal(true)
  const [busy, setBusy] = createSignal(false)
  const [error, setError] = createSignal<string>()
  const [deleteOpen, setDeleteOpen] = createSignal(false)
  const [tasksOpen, setTasksOpen] = createSignal(true)
  const selectedInstance = createMemo(() => { const current = selection(); return current.kind === "instance" ? host()?.instances.find(instance => instance.id === current.id) : undefined })

  async function refresh(silent = false) {
    const api = client(); if (!api) return
    if (!silent) setLoading(true)
    try {
      const [nextHost, nextImages, nextEvents] = await Promise.all([
        api.result<HostSnapshot>("/api/1.0"),
        api.result<ImageArtifact[]>("/api/1.0/images"),
        api.result<{ events: Event[] }>("/api/1.0/events?after=0").catch(() => ({ events: [] })),
      ])
      setHost(normalizeHostSnapshot(nextHost)); setImages(nextImages); setEvents(nextEvents.events); setError(undefined)
    } catch (cause) { setError(cause instanceof Error ? cause.message : "The management gateway is unavailable.") }
    finally { setLoading(false) }
  }

  onMount(() => {
    try { setClient(new ApiClient(consumeFragmentToken())) } catch (cause) { setError(cause instanceof Error ? cause.message : "A management credential is required."); setLoading(false); return }
    void refresh()
    const timer = setInterval(() => void refresh(true), 5000)
    onCleanup(() => clearInterval(timer))
  })

  function navigate(next: Selection, nextTab: Tab = "summary") {
    setSelection(next); setTab(nextTab); setError(undefined)
    const object = next.kind === "instance" ? `instance:${next.id}` : next.kind
    history.replaceState(null, "", `/?object=${encodeURIComponent(object)}&tab=${nextTab}`)
  }

  async function mutate(method: string, path: string, body?: unknown) {
    const api = client(); if (!api) return false
    setBusy(true); setError(undefined)
    try { await api.mutate(method, path, body); await refresh(true); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : "The host action failed."); return false }
    finally { setBusy(false) }
  }

  async function cancelOperation(id: string) {
    const api = client(); if (!api) return
    setBusy(true); setError(undefined)
    try { await api.cancelOperation(id); await refresh(true) } catch (cause) { setError(cause instanceof Error ? cause.message : "The operation could not be cancelled.") } finally { setBusy(false) }
  }

  const tabs = createMemo<Tab[]>(() => selection().kind === "operations" ? ["summary"] : selection().kind === "images" ? ["summary", "manage"] : ["summary", "monitor", "manage"])
  const title = createMemo(() => { const current = selection(); return current.kind === "instance" ? selectedInstance()?.name ?? `Instance ${current.id}` : current.kind === "host" ? "Host" : titleCase(current.kind) })

  const content = <div class="grid h-full min-h-0 grid-rows-[auto_auto_minmax(0,1fr)] overflow-hidden">
    <ObjectHeader selection={selection()} title={title()} instance={selectedInstance()} busy={busy()} refresh={() => void refresh()} mutate={mutate} />
    <Show when={tabs().length > 1}><Tabs value={tab()} onChange={value => navigate(selection(), value as Tab)} class="border-b px-4 md:px-7"><TabsList aria-label="Object views" class="h-11 bg-transparent p-0"><For each={tabs()}>{name => <TabsTrigger value={name} data-testid={`tab-${name}`}>{titleCase(name)}</TabsTrigger>}</For></TabsList></Tabs></Show>
    <ScrollArea class="min-h-0"><div class="px-4 py-5 md:px-7 md:py-6">
      <Show when={error()}><Alert variant="destructive" class="mb-5"><TriangleAlertIcon /><AlertTitle>Management request failed</AlertTitle><AlertDescription>{error()}</AlertDescription></Alert></Show>
      <Show when={host()} fallback={<div class="grid min-h-64 place-items-center text-sm text-muted-foreground">{loading() ? "Loading host inventory…" : "Host inventory is unavailable."}</div>}>{snapshot => <Switch>
        <Match when={selection().kind === "host"}><HostView host={snapshot()} tab={tab()} events={events()} busy={busy()} mutate={mutate} selectInstance={id => navigate({ kind: "instance", id })} /></Match>
        <Match when={selection().kind === "instance"}><InstanceView host={snapshot()} instance={selectedInstance()} images={images()} tab={tab()} busy={busy()} mutate={mutate} onDelete={() => setDeleteOpen(true)} /></Match>
        <Match when={selection().kind === "images"}><ImagesView images={images()} generation={snapshot().generation} tab={tab()} busy={busy()} mutate={mutate} /></Match>
        <Match when={selection().kind === "operations"}><OperationsView operations={snapshot().operations ?? []} busy={busy()} cancel={cancelOperation} /></Match>
      </Switch>}</Show>
    </div></ScrollArea>
  </div>

  return <><a class="skip-link" href="#main-content">Skip to main content</a><ConsoleShell
    inventory={() => <Inventory host={host()} images={images()} selection={selection()} navigate={navigate} />}
    status={<><Badge variant={host()?.health === "healthy" ? "default" : "outline"}>{host() ? titleCase(host()!.health) : "Connecting"}</Badge><span class="hidden text-xs text-console-header-foreground/70 lg:inline">Local administration</span></>}
  >
    <Show when={tasksOpen()} fallback={<div class="grid h-full min-h-0 grid-rows-[minmax(0,1fr)_36px]">{content}<TasksBar operations={host()?.operations ?? []} open={false} toggle={() => setTasksOpen(true)} /></div>}>
      <Resizable direction="vertical" class="h-full"><ResizablePanel defaultSize={78} minSize={0.55} class="min-h-0 overflow-hidden">{content}</ResizablePanel><ResizableHandle withHandle /><ResizablePanel defaultSize={22} minSize={0.12} maxSize={0.4} class="min-h-0 overflow-hidden"><RecentTasks operations={host()?.operations ?? []} toggle={() => setTasksOpen(false)} /></ResizablePanel></Resizable>
    </Show>
  </ConsoleShell>
  <Show when={selectedInstance()}>{instance => <DoubleConfirmation open={deleteOpen()} onOpenChange={setDeleteOpen} title={`Delete ${instance().name}?`} description="This removes the kernel instance definition and cannot be undone." confirmation={instance().name} actionLabel="Delete instance" busy={busy()} onConfirm={async () => { if (await mutate("DELETE", `/api/1.0/instances/${instance().id}`, { expected_generation: host()?.generation ?? instance().generation })) { setDeleteOpen(false); navigate({ kind: "host" }) } }} />}</Show>
  </>
}

function Inventory(props: { host?: HostSnapshot; images: ImageArtifact[]; selection: Selection; navigate: (selection: Selection, tab?: Tab) => void }) {
  const item = (label: string, icon: JSX.Element, selected: boolean, action: () => void, badge?: JSX.Element, testId?: string) => <SidebarMenuItem><SidebarMenuButton isActive={selected} onClick={action} data-testid={testId}>{icon}<span>{label}</span><Show when={badge}><SidebarMenuBadge>{badge}</SidebarMenuBadge></Show></SidebarMenuButton></SidebarMenuItem>
  return <nav aria-label="Object inventory">
    <SidebarGroup class="p-0"><SidebarGroupLabel>Infrastructure</SidebarGroupLabel><SidebarGroupContent><SidebarMenu>{item("Host", <ServerIcon />, props.selection.kind === "host", () => props.navigate({ kind: "host" }), undefined, "nav-host")}</SidebarMenu></SidebarGroupContent></SidebarGroup>
    <SidebarGroup class="mt-5 p-0"><SidebarGroupLabel>Kernel instances</SidebarGroupLabel><SidebarGroupContent><SidebarMenu><For each={props.host?.instances ?? []}>{instance => item(instance.name, <BoxIcon />, props.selection.kind === "instance" && props.selection.id === instance.id, () => props.navigate({ kind: "instance", id: instance.id }), <span class={`state-dot ${instance.state}`} aria-label={instance.state} />, `nav-instance-${instance.id}`)}</For><Show when={!props.host?.instances.length}><li class="px-2 py-2 text-xs text-muted-foreground">No kernel instances</li></Show></SidebarMenu></SidebarGroupContent></SidebarGroup>
    <SidebarGroup class="mt-5 p-0"><SidebarGroupLabel>Host resources</SidebarGroupLabel><SidebarGroupContent><SidebarMenu>{item("Images", <ImageIcon />, props.selection.kind === "images", () => props.navigate({ kind: "images" }), props.images.length, "nav-images")}{item("Operations", <Layers3Icon />, props.selection.kind === "operations", () => props.navigate({ kind: "operations" }), props.host?.operations?.length ?? 0, "nav-operations")}</SidebarMenu></SidebarGroupContent></SidebarGroup>
  </nav>
}

function ObjectHeader(props: { selection: Selection; title: string; instance?: Instance; busy: boolean; refresh: () => void; mutate: (method: string, path: string, body?: unknown) => Promise<boolean> }) {
  return <header class="flex min-w-0 flex-col gap-4 border-b px-4 py-4 sm:flex-row sm:items-start sm:justify-between md:px-7 md:py-5"><div class="min-w-0"><p class="flex items-center gap-2 text-xs text-muted-foreground"><Switch><Match when={props.selection.kind === "host"}><ServerIcon class="size-4" /></Match><Match when={props.selection.kind === "instance"}><BoxIcon class="size-4" /></Match><Match when={props.selection.kind === "images"}><ImageIcon class="size-4" /></Match><Match when={props.selection.kind === "operations"}><ActivityIcon class="size-4" /></Match></Switch>{props.selection.kind === "instance" ? "Kernel instances" : "Kernmux"}</p><div class="mt-1 flex items-center gap-3"><h1 data-testid="object-title" class="truncate text-[28px] font-[750] tracking-tight">{props.title}</h1><Show when={props.instance}>{instance => <Badge variant={statusVariant(instance().state) as never}>{titleCase(instance().state)}</Badge>}</Show></div></div><div class="flex flex-wrap items-center gap-2"><Button variant="outline" size="sm" disabled={props.busy} onClick={props.refresh}><RefreshCwIcon />Refresh</Button><Show when={props.instance}>{instance => <><Show when={instance().state === "loaded"}><Button size="sm" disabled={props.busy} onClick={() => void props.mutate("POST", `/api/1.0/instances/${instance().id}/start`, { expected_generation: instance().generation })}>Start</Button></Show><Show when={instance().state === "active"}><Button variant="outline" size="sm" disabled={props.busy} onClick={() => void props.mutate("POST", `/api/1.0/instances/${instance().id}/stop`, { expected_generation: instance().generation, force: false })}>Stop</Button></Show></>}</Show></div></header>
}

function HostView(props: { host: HostSnapshot; tab: Tab; events: Event[]; busy: boolean; mutate: (method: string, path: string, body?: unknown) => Promise<boolean>; selectInstance: (id: number) => void }) {
  return <Switch><Match when={props.tab === "summary"}><HostSummary host={props.host} selectInstance={props.selectInstance} /></Match><Match when={props.tab === "monitor"}><HostMonitor host={props.host} events={props.events} /></Match><Match when={props.tab === "manage"}><HostManage host={props.host} busy={props.busy} mutate={props.mutate} /></Match></Switch>
}

function HostSummary(props: { host: HostSnapshot; selectInstance: (id: number) => void }) {
  const columns: ConsoleColumn[] = [{ id: "name", label: "Instance" }, { id: "state", label: "State" }, { id: "cpu", label: "CPUs" }, { id: "memory", label: "Memory" }, { id: "image", label: "Image", optional: true }]
  const rows = () => props.host.instances.map(instance => ({ id: String(instance.id), search: `${instance.name} ${instance.state} ${instance.resources.cpu_hardware_ids.join(" ")}`, testId: `instance-row-${instance.id}`, cells: { name: <strong>{instance.name}</strong>, state: <Badge variant={statusVariant(instance.state) as never}>{titleCase(instance.state)}</Badge>, cpu: instance.resources.cpu_hardware_ids.join(", ") || "None", memory: formatBytes(instance.resources.memory_bytes), image: instance.image.present ? "Loaded" : "Not loaded" } }))
  return <DetailView testId="host-summary" summary={[{ label: "Host status", value: titleCase(props.host.health), detail: props.host.kernel.multikernel_enabled ? "Multikernel enabled" : "Multikernel unavailable", tone: props.host.health === "healthy" ? "success" : "warning" }, { label: "Control kernel", value: props.host.kernel.release, detail: props.host.topology.architecture }, { label: "CPU capacity", value: `${number.format(props.host.topology.cpus.length)} logical CPUs`, detail: `${props.host.resource_pool.cpu_hardware_ids.length} delegated` }, { label: "Memory capacity", value: formatBytes(props.host.memory.total_bytes), detail: `${formatBytes(props.host.memory.assigned_bytes)} assigned` }]}>
    <Show when={props.host.diagnostics?.length}><div class="grid gap-2"><For each={props.host.diagnostics}>{diagnostic => <Alert variant={diagnostic.severity === "error" ? "destructive" : "default"}><TriangleAlertIcon /><AlertTitle>{titleCase(diagnostic.severity)}</AlertTitle><AlertDescription>{diagnostic.message}<Show when={diagnostic.detail}><span>{diagnostic.detail}</span></Show></AlertDescription></Alert>}</For></div></Show>
    <div class="grid gap-5 min-[1000px]:grid-cols-12"><DetailSection class="min-[1000px]:col-span-5" title="Host information" meta={`Generation ${props.host.generation}`}><DetailList items={[{ label: "Architecture", value: props.host.topology.architecture }, { label: "NUMA nodes", value: props.host.topology.numa_nodes.length }, { label: "Capabilities", value: props.host.capabilities.map(titleCase).join(", ") || "None advertised" }, { label: "Available pool CPUs", value: props.host.resource_pool.available_cpu_hardware_ids.join(", ") || "None" }]} columns={2} /></DetailSection><DetailSection class="min-[1000px]:col-span-7" title="Current allocation" meta="Authoritative snapshot"><Allocation host={props.host} /></DetailSection></div>
    <DetailSection title="Kernel instances" meta={`${props.host.instances.length} defined`}><DataTableView columns={columns} rows={rows()} filterPlaceholder="Filter instances…" emptyTitle="No kernel instances" emptyDescription="Create an instance from the Manage tab." onRowClick={id => props.selectInstance(Number(id))} testId="instance-inventory" /></DetailSection>
  </DetailView>
}

function Allocation(props: { host: HostSnapshot }) {
  const memoryPercent = Math.min(100, props.host.memory.assignable_bytes ? props.host.memory.assigned_bytes / props.host.memory.assignable_bytes * 100 : 0)
  const cpuAssigned = new Set(props.host.instances.flatMap(instance => instance.resources.cpu_hardware_ids)).size
  const cpuPercent = Math.min(100, props.host.resource_pool.cpu_hardware_ids.length ? cpuAssigned / props.host.resource_pool.cpu_hardware_ids.length * 100 : 0)
  return <div class="grid gap-5 px-4 py-4"><div><div class="mb-2 flex justify-between gap-4 text-sm"><strong>Delegated CPU allocation</strong><span class="tabular-nums text-muted-foreground">{cpuAssigned} assigned of {props.host.resource_pool.cpu_hardware_ids.length}</span></div><Progress value={cpuPercent} aria-label="Delegated CPU allocation" /></div><div><div class="mb-2 flex justify-between gap-4 text-sm"><strong>Assignable memory allocation</strong><span class="tabular-nums text-muted-foreground">{formatBytes(props.host.memory.assigned_bytes)} of {formatBytes(props.host.memory.assignable_bytes)}</span></div><Progress value={memoryPercent} aria-label="Assignable memory allocation" /></div></div>
}

function HostMonitor(props: { host: HostSnapshot; events: Event[] }) {
  const owner = (hardwareId: number) => props.host.instances.find(instance => instance.resources.cpu_hardware_ids.includes(hardwareId))?.name ?? (props.host.resource_pool.available_cpu_hardware_ids.includes(hardwareId) ? "Free" : "Control kernel")
  return <div class="grid gap-6" data-testid="host-monitor"><DetailSection title="Resource allocation" meta="Current snapshot"><Allocation host={props.host} /></DetailSection><DetailSection title="CPU and NUMA topology" meta={`${props.host.topology.cpus.length} logical CPUs`}><div class="grid gap-3 p-3 sm:grid-cols-2 xl:grid-cols-4"><For each={props.host.topology.cpus}>{cpu => <div class="grid gap-2 rounded-md border bg-card p-3 text-xs"><div class="flex items-center justify-between"><strong>CPU {cpu.hardware_id}</strong><Badge variant={cpu.online ? "outline" : "destructive"}>{cpu.online ? "Online" : "Offline"}</Badge></div><span class="text-muted-foreground">Package {cpu.package_id} · Core {cpu.core_id} · Thread {cpu.thread_index}</span><span>NUMA {cpu.numa_node}</span><strong class="text-primary">{owner(cpu.hardware_id)}</strong></div>}</For></div></DetailSection><div class="grid gap-5 min-[1000px]:grid-cols-2"><Transactions host={props.host} /><Events events={props.events} /></div><Devices host={props.host} /></div>
}

function Transactions(props: { host: HostSnapshot }) { return <DetailSection title="Transactions" meta={`${props.host.transactions?.length ?? 0} retained`}><Show when={props.host.transactions?.length} fallback={<EmptyState compact title="No transactions" description="Resource transactions will appear here." />}><Table><TableHeader><TableRow><TableHead>ID</TableHead><TableHead>State</TableHead><TableHead>Generation</TableHead></TableRow></TableHeader><TableBody><For each={props.host.transactions}>{transaction => <TableRow><TableCell class="font-semibold">{transaction.id}</TableCell><TableCell><Badge variant={statusVariant(transaction.state) as never}>{titleCase(transaction.state)}</Badge></TableCell><TableCell>{transaction.generation_before ?? "—"} → {transaction.generation_after ?? "—"}<Show when={transaction.diagnostics?.length}><p class="mt-1 text-xs text-destructive">{transaction.diagnostics?.map(item => item.message).join("; ")}</p></Show></TableCell></TableRow>}</For></TableBody></Table></Show></DetailSection> }
function Events(props: { events: Event[] }) { return <DetailSection title="Events" meta={`${props.events.length} retained`}><Show when={props.events.length} fallback={<EmptyState compact title="No recent events" description="Host invalidation events will appear here." />}><Table><TableHeader><TableRow><TableHead>Sequence</TableHead><TableHead>Event</TableHead><TableHead>Resource</TableHead></TableRow></TableHeader><TableBody><For each={props.events}>{event => <TableRow><TableCell>{event.sequence}</TableCell><TableCell>{titleCase(event.kind)}</TableCell><TableCell>{event.resource ? `${titleCase(event.resource.kind)} ${event.resource.id}` : "Host"}</TableCell></TableRow>}</For></TableBody></Table></Show></DetailSection> }
function Devices(props: { host: HostSnapshot }) { return <DetailSection title="PCI and IOMMU devices" meta={`${props.host.resource_pool.devices.length} delegated`}><Show when={props.host.resource_pool.devices.length} fallback={<EmptyState compact title="No delegated devices" description="PCI devices delegated to the resource pool will appear here." />}><Table><TableHeader><TableRow><TableHead>PCI ID</TableHead><TableHead>Pool</TableHead><TableHead>Vendor / Device</TableHead><TableHead>IOMMU group</TableHead><TableHead>Availability</TableHead></TableRow></TableHeader><TableBody><For each={props.host.resource_pool.devices}>{device => <TableRow><TableCell class="font-semibold">{device.pci_id}</TableCell><TableCell>{device.pool_name}</TableCell><TableCell>{device.vendor_id === undefined ? "—" : `0x${device.vendor_id.toString(16)}`} / {device.device_id === undefined ? "—" : `0x${device.device_id.toString(16)}`}</TableCell><TableCell>{device.iommu_group ?? "—"}<Show when={device.iommu_group_members?.length}><span class="block text-xs text-muted-foreground">{device.iommu_group_members?.join(", ")}</span></Show></TableCell><TableCell>{props.host.resource_pool.available_device_ids.includes(device.pci_id) ? "Available" : "Assigned"}</TableCell></TableRow>}</For></TableBody></Table></Show></DetailSection> }

function HostManage(props: { host: HostSnapshot; busy: boolean; mutate: (method: string, path: string, body?: unknown) => Promise<boolean> }) {
  const [poolCpus, setPoolCpus] = createSignal(props.host.resource_pool.cpu_hardware_ids.join(",")); const [poolMemory, setPoolMemory] = createSignal(String(props.host.memory.assignable_bytes / 2 ** 30))
  const [id, setId] = createSignal(""); const [name, setName] = createSignal(""); const [cpus, setCpus] = createSignal(""); const [memory, setMemory] = createSignal("")
  const poolValid = () => parseCpuList(poolCpus()).length > 0 && Number(poolMemory()) > 0
  const instanceValid = () => Number.isInteger(Number(id())) && Number(id()) >= 0 && name().trim().length > 0 && parseCpuList(cpus()).length > 0 && Number(memory()) > 0
  return <div class="grid gap-7" data-testid="host-manage"><FormFlow title="Resource pool" description="Delegate CPU and memory from the control kernel." canReview={poolValid} busy={props.busy} applyLabel="Apply resource pool" review={() => [{ label: "CPU hardware IDs", value: poolCpus() }, { label: "Assignable memory", value: `${poolMemory()} GiB` }]} onApply={() => props.mutate("PUT", "/api/1.0/resource-pool", { expected_generation: props.host.generation, cpu_hardware_ids: parseCpuList(poolCpus()), memory_bytes: Number(poolMemory()) * 2 ** 30 })}><FieldGroup class="grid gap-4 min-[760px]:grid-cols-2"><TextField id="pool-cpus" label="CPU hardware IDs" value={poolCpus()} onInput={setPoolCpus} error={poolCpus() && !parseCpuList(poolCpus()).length ? "Use IDs or ranges such as 2-7,10." : undefined} /><TextField id="pool-memory" label="Memory (GiB)" type="number" value={poolMemory()} onInput={setPoolMemory} error={Number(poolMemory()) <= 0 ? "Memory must be greater than zero." : undefined} /></FieldGroup></FormFlow>
    <FormFlow title="Create kernel instance" description="Create an instance from resources already in the pool." canReview={instanceValid} busy={props.busy} applyLabel="Create instance" review={() => [{ label: "Instance ID", value: id() }, { label: "Name", value: name() }, { label: "CPU hardware IDs", value: cpus() }, { label: "Memory", value: `${memory()} GiB` }]} onApply={() => props.mutate("POST", "/api/1.0/instances", { expected_generation: props.host.generation, id: Number(id()), name: name().trim(), cpu_hardware_ids: parseCpuList(cpus()), memory_bytes: Number(memory()) * 2 ** 30 })}><FieldGroup class="grid gap-4 min-[760px]:grid-cols-2"><TextField id="instance-id" label="Instance ID" type="number" value={id()} onInput={setId} /><TextField id="instance-name" label="Name" value={name()} onInput={setName} /><TextField id="instance-cpus" label="CPU hardware IDs" value={cpus()} onInput={setCpus} description={`Available: ${props.host.resource_pool.available_cpu_hardware_ids.join(", ") || "none"}`} /><TextField id="instance-memory" label="Memory (GiB)" type="number" value={memory()} onInput={setMemory} /></FieldGroup></FormFlow></div>
}

function InstanceView(props: { host: HostSnapshot; instance?: Instance; images: ImageArtifact[]; tab: Tab; busy: boolean; mutate: (method: string, path: string, body?: unknown) => Promise<boolean>; onDelete: () => void }) {
  return <Show when={props.instance} fallback={<EmptyState title="Instance not found" description="Select another instance from the inventory." />}>{instance => <Switch><Match when={props.tab === "summary"}><InstanceSummary instance={instance()} operations={props.host.operations ?? []} /></Match><Match when={props.tab === "monitor"}><InstanceMonitor host={props.host} instance={instance()} /></Match><Match when={props.tab === "manage"}><InstanceManage instance={instance()} images={props.images} busy={props.busy} mutate={props.mutate} onDelete={props.onDelete} /></Match></Switch>}</Show>
}
function InstanceSummary(props: { instance: Instance; operations: Operation[] }) { const activity = () => props.operations.filter(operation => operation.affected_resources?.some(resource => resource.kind === "instance" && resource.id === String(props.instance.id))); return <DetailView testId="instance-summary" summary={[{ label: "Runtime state", value: titleCase(props.instance.state), detail: `Generation ${props.instance.generation}`, tone: props.instance.state === "active" ? "success" : "default" }, { label: "CPU allocation", value: `${props.instance.resources.cpu_hardware_ids.length} logical CPUs`, detail: props.instance.resources.cpu_hardware_ids.join(", ") || "None" }, { label: "Memory allocation", value: formatBytes(props.instance.resources.memory_bytes), detail: props.instance.resources.memory_region ?? "Managed region" }, { label: "Kernel image", value: props.instance.image.present ? "Loaded" : "Not loaded", detail: `${props.instance.resources.device_ids.length} devices` }]}><div class="grid gap-5 min-[1000px]:grid-cols-2"><DetailSection title="Configuration"><DetailList columns={2} items={[{ label: "Instance ID", value: props.instance.id }, { label: "CPU hardware IDs", value: props.instance.resources.cpu_hardware_ids.join(", ") || "None" }, { label: "Memory base", value: props.instance.resources.memory_base === undefined ? "—" : `0x${props.instance.resources.memory_base.toString(16)}` }, { label: "Devices", value: props.instance.resources.device_ids.join(", ") || "None" }]} /></DetailSection><DetailSection title="Recent activity" meta={`${activity().length} operations`}><OperationTable operations={activity()} /></DetailSection></div></DetailView> }
function InstanceMonitor(props: { host: HostSnapshot; instance: Instance }) { const cpus = () => props.host.topology.cpus.filter(cpu => props.instance.resources.cpu_hardware_ids.includes(cpu.hardware_id)); return <div class="grid gap-6" data-testid="instance-monitor"><DetailSection title="Assigned CPU topology" meta={`${cpus().length} logical CPUs`}><div class="grid gap-3 p-3 sm:grid-cols-2 xl:grid-cols-4"><For each={cpus()}>{cpu => <div class="rounded-md border bg-card p-3 text-xs"><strong>CPU {cpu.hardware_id}</strong><p class="mt-2 text-muted-foreground">Package {cpu.package_id} · Core {cpu.core_id} · Thread {cpu.thread_index}</p><p class="mt-1">NUMA {cpu.numa_node}</p></div>}</For></div></DetailSection><DetailSection title="Assigned resources"><DetailList columns={2} items={[{ label: "Memory", value: formatBytes(props.instance.resources.memory_bytes) }, { label: "Memory region", value: props.instance.resources.memory_region ?? "—" }, { label: "Devices", value: props.instance.resources.device_ids.join(", ") || "None" }, { label: "Runtime state", value: titleCase(props.instance.state) }]} /></DetailSection><Alert><ActivityIcon /><AlertTitle>Utilization history is unavailable</AlertTitle><AlertDescription>The current host API reports authoritative allocation and topology, but does not expose CPU, memory, disk, or network utilization samples.</AlertDescription></Alert></div> }

function InstanceManage(props: { instance: Instance; images: ImageArtifact[]; busy: boolean; mutate: (method: string, path: string, body?: unknown) => Promise<boolean>; onDelete: () => void }) {
  const [cpus, setCpus] = createSignal(props.instance.resources.cpu_hardware_ids.join(",")); const [memory, setMemory] = createSignal(String(props.instance.resources.memory_bytes / 2 ** 30)); const [devices, setDevices] = createSignal(props.instance.resources.device_ids.join(",")); const [kernel, setKernel] = createSignal(""); const [initrd, setInitrd] = createSignal(""); const [commandLine, setCommandLine] = createSignal("")
  const kernels = () => props.images.filter(image => image.kind === "kernel"); const initrds = () => props.images.filter(image => image.kind === "initrd")
  return <div class="grid gap-7" data-testid="instance-manage"><DetailSection title="Lifecycle" meta={titleCase(props.instance.state)}><div class="flex flex-wrap gap-2 p-4"><Show when={props.instance.state === "loaded"}><Button disabled={props.busy} onClick={() => void props.mutate("POST", `/api/1.0/instances/${props.instance.id}/start`, { expected_generation: props.instance.generation })}>Start instance</Button></Show><Show when={props.instance.state === "active"}><Button variant="outline" disabled={props.busy} onClick={() => void props.mutate("POST", `/api/1.0/instances/${props.instance.id}/stop`, { expected_generation: props.instance.generation, force: false })}>Stop instance</Button></Show><Show when={props.instance.image.present && props.instance.state !== "active"}><Button variant="outline" disabled={props.busy} onClick={() => void props.mutate("POST", `/api/1.0/instances/${props.instance.id}/unload`, { expected_generation: props.instance.generation })}>Unload image</Button></Show><Button variant="destructive" data-testid="action-delete" disabled={props.busy || props.instance.state === "active"} onClick={props.onDelete}><Trash2Icon />Delete instance</Button></div></DetailSection>
    <FormFlow title="Resource assignment" description="Review CPU, memory, and device changes before applying." canReview={() => parseCpuList(cpus()).length > 0 && Number(memory()) > 0} busy={props.busy} applyLabel="Apply resources" review={() => [{ label: "CPU hardware IDs", value: cpus() }, { label: "Memory", value: `${memory()} GiB` }, { label: "Devices", value: devices() || "None" }]} onApply={() => props.mutate("PATCH", `/api/1.0/instances/${props.instance.id}`, { expected_generation: props.instance.generation, cpu_hardware_ids: parseCpuList(cpus()), memory_bytes: Number(memory()) * 2 ** 30, device_ids: devices().split(",").map(value => value.trim()).filter(Boolean), dry_run: false })}><FieldGroup class="grid gap-4 min-[760px]:grid-cols-2"><TextField id="edit-cpus" label="CPU hardware IDs" value={cpus()} onInput={setCpus} /><TextField id="edit-memory" label="Memory (GiB)" type="number" value={memory()} onInput={setMemory} /><TextField id="edit-devices" label="Device IDs" value={devices()} onInput={setDevices} description="Comma-separated PCI resource IDs." /></FieldGroup></FormFlow>
    <FormFlow title="Managed kernel image" description="Load a verified kernel and optional initrd." canReview={() => Boolean(kernel())} busy={props.busy} applyLabel="Load image" review={() => [{ label: "Kernel", value: kernel() }, { label: "Initrd", value: initrd() || "None" }, { label: "Command line", value: commandLine() || "Default" }]} onApply={() => props.mutate("POST", `/api/1.0/instances/${props.instance.id}/load-image`, { expected_generation: props.instance.generation, kernel_id: kernel(), initrd_id: initrd() || undefined, command_line: commandLine() || undefined })}><FieldGroup class="grid gap-4 min-[760px]:grid-cols-2"><SelectField id="kernel-image" label="Kernel image" value={kernel()} onChange={setKernel} options={kernels().map(image => [image.id, image.id])} /><SelectField id="initrd-image" label="Initrd image" value={initrd()} onChange={setInitrd} options={[["", "None"], ...initrds().map(image => [image.id, image.id] as [string, string])]} /><TextField id="kernel-command-line" label="Kernel command line" value={commandLine()} onInput={setCommandLine} description="Optional; for example console=mktty0." /></FieldGroup><Show when={!kernels().length}><FieldError>No kernel images are available. Import one from Images → Manage.</FieldError></Show></FormFlow></div>
}

function ImagesView(props: { images: ImageArtifact[]; generation: number; tab: Tab; busy: boolean; mutate: (method: string, path: string, body?: unknown) => Promise<boolean> }) {
  const [kind, setKind] = createSignal("kernel"); const [source, setSource] = createSignal(""); const [expected, setExpected] = createSignal("")
  const columns: ConsoleColumn[] = [{ id: "id", label: "Artifact ID" }, { id: "kind", label: "Type" }, { id: "bytes", label: "Size" }, { id: "schema", label: "Schema", optional: true }]
  const rows: ConsoleRow[] = props.images.map(image => ({ id: image.id, search: `${image.id} ${image.kind}`, cells: { id: <span class="break-all font-mono text-xs">{image.id}</span>, kind: titleCase(image.kind), bytes: formatBytes(image.bytes), schema: image.schema_version } }))
  return <Switch><Match when={props.tab === "summary"}><DataTableView columns={columns} rows={rows} filterPlaceholder="Filter images…" emptyTitle="No managed images" emptyDescription="Import a verified kernel or initrd from the Manage tab." testId="images-table" /></Match><Match when={props.tab === "manage"}><FormFlow title="Import image from host" description="Copy an administrator-controlled file into immutable image storage." canReview={() => source().startsWith("/")} busy={props.busy} applyLabel="Import image" review={() => [{ label: "Type", value: titleCase(kind()) }, { label: "Source path", value: source() }, { label: "Expected SHA-256", value: expected() || "Not supplied" }]} onApply={() => props.mutate("POST", "/api/1.0/images", { expected_generation: props.generation, kind: kind(), source_path: source(), expected_id: expected() || undefined })}><FieldGroup class="grid gap-4 min-[760px]:grid-cols-2"><SelectField id="image-kind" label="Image type" value={kind()} onChange={setKind} options={[["kernel", "Kernel"], ["initrd", "Initrd"]]} /><TextField id="image-source" label="Source path" value={source()} onInput={setSource} error={source() && !source().startsWith("/") ? "Use an absolute host path." : undefined} /><TextField id="image-digest" label="Expected SHA-256" value={expected()} onInput={setExpected} description="Optional integrity precondition." /></FieldGroup></FormFlow></Match></Switch>
}

function OperationsView(props: { operations: Operation[]; busy: boolean; cancel: (id: string) => void }) {
  const columns: ConsoleColumn[] = [{ id: "task", label: "Task" }, { id: "target", label: "Target" }, { id: "state", label: "State" }, { id: "progress", label: "Progress" }, { id: "started", label: "Started", optional: true }, { id: "result", label: "Result", optional: true }]
  const rows = props.operations.slice().reverse().map(operation => ({ id: operation.id, search: `${operation.kind} ${operation.state} ${operation.id}`, cells: { task: <div><strong>{titleCase(operation.kind)}</strong><span class="block max-w-56 truncate text-xs text-muted-foreground">{operation.id}</span></div>, target: operation.affected_resources?.map(resource => `${titleCase(resource.kind)} ${resource.id}`).join(", ") || "Host", state: <Badge variant={statusVariant(operation.state) as never}>{titleCase(operation.state)}</Badge>, progress: <div class="min-w-28"><Progress value={operation.progress_percent ?? (operation.state === "succeeded" ? 100 : 0)} /><span class="mt-1 block text-xs text-muted-foreground">{operation.progress_percent === undefined ? "—" : `${operation.progress_percent}%`}</span></div>, started: formatDate(operation.created_at), result: <div><Show when={["queued", "running"].includes(operation.state)} fallback={operation.error?.message ?? (operation.completed_at ? formatDate(operation.completed_at) : "—")}><Button variant="outline" size="xs" disabled={props.busy} onClick={event => { event.stopPropagation(); props.cancel(operation.id) }}>Cancel</Button></Show><Show when={operation.actor}><span class="block text-xs text-muted-foreground">{operation.actor?.label ?? `UID ${operation.actor?.uid}`}</span></Show></div> } }))
  return <DataTableView columns={columns} rows={rows} filterPlaceholder="Filter operations…" emptyTitle="No host operations" emptyDescription="Lifecycle and resource operations will appear here." testId="operations-table" />
}
function OperationTable(props: { operations: Operation[] }) { return <Show when={props.operations.length} fallback={<EmptyState compact title="No related activity" description="Instance operations will appear here." />}><Table><TableHeader><TableRow><TableHead>Task</TableHead><TableHead>State</TableHead><TableHead>Started</TableHead></TableRow></TableHeader><TableBody><For each={props.operations.slice().reverse()}>{operation => <TableRow><TableCell>{titleCase(operation.kind)}</TableCell><TableCell><Badge variant={statusVariant(operation.state) as never}>{titleCase(operation.state)}</Badge></TableCell><TableCell>{formatDate(operation.created_at)}</TableCell></TableRow>}</For></TableBody></Table></Show> }

function RecentTasks(props: { operations: Operation[]; toggle: () => void }) { return <div class="grid h-full min-h-0 grid-rows-[36px_minmax(0,1fr)] bg-card" data-testid="recent-tasks"><TasksBar operations={props.operations} open toggle={props.toggle} /><div class="min-h-0 overflow-auto"><Show when={props.operations.length} fallback={<p class="px-4 py-3 text-xs text-muted-foreground">No recent host tasks</p>}><OperationTable operations={props.operations.slice(-8)} /></Show></div></div> }
function TasksBar(props: { operations: Operation[]; open: boolean; toggle: () => void }) { return <div class="flex items-center justify-between border-b px-4 text-xs"><strong>Recent Tasks</strong><div class="flex items-center gap-3"><span class="text-muted-foreground">{props.operations.length} tasks</span><Button variant="outline" size="xs" onClick={props.toggle}>{props.open ? "Hide" : "Show"}</Button></div></div> }

function TextField(props: { id: string; label: string; value: string; onInput: (value: string) => void; type?: string; description?: string; error?: string }) { return <Field data-invalid={Boolean(props.error)}><FieldLabel for={props.id}>{props.label}</FieldLabel><Input id={props.id} name={props.id} type={props.type ?? "text"} value={props.value} autocomplete="off" aria-invalid={Boolean(props.error)} onInput={event => props.onInput(event.currentTarget.value)} /><Show when={props.description}><FieldDescription>{props.description}</FieldDescription></Show><Show when={props.error}><FieldError>{props.error}</FieldError></Show></Field> }
function SelectField(props: { id: string; label: string; value: string; onChange: (value: string) => void; options: Array<[string, string]> }) { return <Field><FieldLabel for={props.id}>{props.label}</FieldLabel><NativeSelect id={props.id} name={props.id} value={props.value} onChange={event => props.onChange(event.currentTarget.value)} class="w-full"><For each={props.options}>{option => <NativeSelectOption value={option[0]}>{option[1]}</NativeSelectOption>}</For></NativeSelect></Field> }
function formatDate(value: string) { const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : dateTime.format(date) }
