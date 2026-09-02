import { For, Match, Show, Switch, createMemo, createSignal, lazy, onCleanup, onMount, type JSX } from "solid-js"
import ChevronDownIcon from "lucide-solid/icons/chevron-down"
import CircleAlertIcon from "lucide-solid/icons/circle-alert"
import PlayIcon from "lucide-solid/icons/play"
import PowerIcon from "lucide-solid/icons/power"
import RefreshCwIcon from "lucide-solid/icons/refresh-cw"
import ServerIcon from "lucide-solid/icons/server"
import SquareIcon from "lucide-solid/icons/square"
import SquareTerminalIcon from "lucide-solid/icons/square-terminal"

import { availableActions } from "@/actions"
import { ApiError, KernmuxApi } from "@/api"
import { initializeToken } from "@/auth"
import { ConsoleShell, StateBadge } from "@/components/console-shell"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogPortal, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb"
import { Button, buttonVariants } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select"
import { Progress } from "@/components/ui/progress"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { bytes, label, timestamp } from "@/format"
import type { AcceptedEnvelope, EventPage, HostSnapshot, ImageArtifact, Instance, Operation, OsImage } from "@/model"
import { parseRoute, routeHref, type Route, type ViewTab } from "@/route"

const InstanceConsole = lazy(() => import("@/components/instance-console").then((module) => ({ default: module.InstanceConsole })))

export default function App() {
  const credential = initializeToken()
  const api = credential ? new KernmuxApi(credential) : null
  const [route, setRoute] = createSignal<Route>(parseRoute(window.location.hash))
  const [host, setHost] = createSignal<HostSnapshot>()
  const [images, setImages] = createSignal<ImageArtifact[]>([])
  const [osImages, setOsImages] = createSignal<OsImage[]>([])
  const [osImageGeneration, setOsImageGeneration] = createSignal(0)
  const [operations, setOperations] = createSignal<Operation[]>([])
  const [events, setEvents] = createSignal<EventPage>({ events: [], latest_sequence: 0, overflowed: false })
  const [loading, setLoading] = createSignal(Boolean(api))
  const [error, setError] = createSignal<ApiError | Error>()
  const [busyInstance, setBusyInstance] = createSignal<number>()

  const refresh = async () => {
    if (!api) return
    setLoading(!host())
    setError()
    try {
      const [hostResult, imageResult, osImageResult, operationResult, eventResult] = await Promise.all([api.host(), api.images(), api.osImages(), api.operations(), api.events()])
      setHost(hostResult.data)
      setImages(imageResult.data)
      setOsImages(osImageResult.data)
      setOsImageGeneration(osImageResult.generation)
      setOperations(operationResult.data)
      setEvents(eventResult.data)
    } catch (cause) {
      setError(cause instanceof Error ? cause : new Error("Unable to load the host."))
    } finally {
      setLoading(false)
    }
  }

  onMount(() => {
    const changed = () => setRoute(parseRoute(window.location.hash))
    window.addEventListener("hashchange", changed)
    onCleanup(() => window.removeEventListener("hashchange", changed))
    void refresh()
  })

  async function finishMutation(owner: number | undefined, request: () => Promise<AcceptedEnvelope>): Promise<boolean> {
    if (!api) return false
    setBusyInstance(owner)
    setError()
    try {
      const accepted = await request()
      let operation = accepted.operation
      for (let attempt = 0; attempt < 60 && (operation.state === "queued" || operation.state === "running"); attempt += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 250))
        operation = (await api.operation(operation.id)).data
      }
      if (operation.state !== "succeeded") {
        throw new Error(operation.error?.message ?? "The host operation did not complete successfully.")
      }
      await refresh()
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause : new Error("The host operation failed."))
      return false
    } finally {
      setBusyInstance()
    }
  }

  const lifecycle = (instance: Instance, action: "start" | "stop" | "unload") => finishMutation(instance.id, () => api!.lifecycle(instance.id, action, instance.generation))

  if (!credential) return <CredentialRequired />

  if (!loading() && error() && !host()) return <FatalError error={error()!} onRetry={refresh} />

  return (
    <Show when={!loading() && host()} fallback={<LoadingShell />}>
      {(snapshot) => (
        <ConsoleShell route={route()} instances={snapshot().instances} operations={operations()}>
          <Show when={error()}>{(value) => <ErrorBanner error={value()} onRetry={refresh} />}</Show>
          <Switch>
            <Match when={route().kind === "host"}><HostView host={snapshot()} route={route() as Extract<Route, { kind: "host" }>} operations={operations()} events={events()} /></Match>
            <Match when={route().kind === "instances"}><InstancesView host={snapshot()} images={osImages()} /></Match>
            <Match when={route().kind === "instance"}>
              <InstanceRouteView host={snapshot()} images={images()} credential={credential} route={route() as Extract<Route, { kind: "instance" }>} operations={operations()} busy={busyInstance()} onLifecycle={lifecycle} onUpdate={(instance, input) => finishMutation(instance.id, () => api!.updateInstance(instance.id, { expected_generation: instance.generation, ...input }))} onLoad={(instance, input) => finishMutation(instance.id, () => api!.loadManagedImage(instance.id, { expected_generation: instance.generation, ...input }))} onDelete={async (instance) => { const succeeded = await finishMutation(instance.id, () => api!.deleteInstance(instance.id, instance.generation)); if (succeeded) window.location.hash = "#/instances"; return succeeded }} />
            </Match>
            <Match when={route().kind === "images"}><ImagesView images={osImages()} generation={osImageGeneration()} busy={busyInstance() !== undefined} onUpload={(input) => finishMutation(0, () => api!.uploadOsImage(input))} /></Match>
          </Switch>
        </ConsoleShell>
      )}
    </Show>
  )
}

function CredentialRequired() {
  return <main class="grid min-h-svh place-items-center bg-background p-6 text-foreground"><section class="w-full max-w-lg border-y py-8"><div class="flex items-center gap-3"><span class="grid size-10 place-items-center rounded-md bg-muted"><ServerIcon /></span><div><h1 class="text-xl font-bold">Kernmux Host Client</h1><p class="text-sm text-muted-foreground">Authentication required</p></div></div><Alert class="mt-6"><CircleAlertIcon /><AlertTitle>Open an authorized session</AlertTitle><AlertDescription>Generate the local Host Client URL on the Kernmux host and open it in this browser. Credentials are accepted only from the URL fragment and are removed immediately after use.</AlertDescription></Alert></section></main>
}

function LoadingShell() {
  return <main class="grid min-h-svh grid-cols-[248px_1fr] grid-rows-[52px_1fr] bg-background"><div class="col-span-2 bg-console-header" /><aside class="border-r p-4"><Skeleton class="h-5 w-24" /><Skeleton class="mt-5 h-8 w-full" /><Skeleton class="mt-2 h-8 w-4/5" /></aside><section class="p-8"><Skeleton class="h-8 w-72" /><Skeleton class="mt-8 h-40 w-full" /><Skeleton class="mt-5 h-64 w-full" /></section></main>
}

function FatalError(props: { error: Error; onRetry: () => Promise<void> }) {
  const unauthorized = () => props.error instanceof ApiError && props.error.status === 401
  return <main class="grid min-h-svh place-items-center bg-background p-6 text-foreground"><section class="w-full max-w-lg border-y py-8"><h1 class="text-xl font-bold">{unauthorized() ? "Session unauthorized" : "Host unavailable"}</h1><p class="mt-2 text-sm text-muted-foreground">{unauthorized() ? "Open a newly generated authorized Host Client URL." : props.error.message}</p><Button class="mt-6" variant="outline" onClick={() => void props.onRetry()}><RefreshCwIcon aria-hidden="true" />Retry connection</Button></section></main>
}

function ErrorBanner(props: { error: Error; onRetry: () => Promise<void> }) {
  const apiError = () => props.error instanceof ApiError ? props.error : null
  return <div class="px-4 pt-4 md:px-8" aria-live="polite"><Alert variant="destructive"><CircleAlertIcon aria-hidden="true" /><AlertTitle>{apiError()?.status === 401 ? "Session unauthorized" : "Host data unavailable"}</AlertTitle><AlertDescription>{props.error.message}</AlertDescription><Button variant="outline" size="xs" onClick={() => void props.onRetry()}><RefreshCwIcon aria-hidden="true" />Retry</Button></Alert></div>
}

function ObjectView(props: { scope: string; title: string; state?: string; tab: ViewTab; base: string; actions?: JSX.Element; children: JSX.Element }) {
  return <div class="min-h-full bg-background px-4 pt-4 pb-7 md:px-8 md:pt-6">
    <Breadcrumb><BreadcrumbList><BreadcrumbItem><BreadcrumbLink href="#/host/summary">Kernmux</BreadcrumbLink></BreadcrumbItem><BreadcrumbSeparator /><BreadcrumbItem><BreadcrumbPage>{props.scope}</BreadcrumbPage></BreadcrumbItem></BreadcrumbList></Breadcrumb>
    <header class="mt-3 flex min-w-0 flex-col gap-4 sm:flex-row sm:items-start sm:justify-between"><div class="min-w-0"><div class="flex flex-wrap items-center gap-3"><h1 class="truncate text-[28px] font-[750]">{props.title}</h1><Show when={props.state}>{(state) => <StateBadge state={state()} />}</Show></div></div><div class="flex shrink-0 items-center gap-2">{props.actions}</div></header>
    <Tabs value={props.tab} onChange={(value) => { window.location.hash = routeHref(props.base === "host" ? { kind: "host", tab: value as ViewTab } : { kind: "instance", id: Number(props.base.split("/")[1]), tab: value as ViewTab }) }} class="mt-3 gap-0"><TabsList><For each={["summary", "monitor", "manage"] as ViewTab[]}>{(tab) => <TabsTrigger value={tab}>{label(tab)}</TabsTrigger>}</For></TabsList></Tabs>
    <main class="pt-5">{props.children}</main>
  </div>
}

function HostView(props: { host: HostSnapshot; route: Extract<Route, { kind: "host" }>; operations: Operation[]; events: EventPage }) {
  return <ObjectView scope="Host" title={props.host.kernel.release} state={props.host.health} tab={props.route.tab} base="host" actions={<Button variant="outline" size="sm" onClick={() => window.location.reload()}><RefreshCwIcon aria-hidden="true" />Refresh</Button>}>
    <Switch>
      <Match when={props.route.tab === "summary"}><HostSummary host={props.host} /></Match>
      <Match when={props.route.tab === "monitor"}><HostMonitor host={props.host} operations={props.operations} events={props.events} /></Match>
      <Match when={props.route.tab === "manage"}><HostManage host={props.host} /></Match>
    </Switch>
  </ObjectView>
}

function HostSummary(props: { host: HostSnapshot }) {
  const assigned = () => props.host.memory.assignable_bytes ? Math.round(props.host.memory.assigned_bytes / props.host.memory.assignable_bytes * 100) : 0
  return <div class="grid gap-6">
    <section class="grid grid-cols-2 border-y min-[900px]:grid-cols-4"><SummaryDatum label="Multikernel" value={props.host.kernel.multikernel_enabled ? "Enabled" : "Unavailable"} detail={props.host.topology.architecture} /><SummaryDatum label="Resource pool" value={`${props.host.resource_pool.cpu_hardware_ids.length} CPUs`} detail={`${bytes(props.host.memory.assignable_bytes)} assignable`} /><SummaryDatum label="Instances" value={String(props.host.instances.length)} detail={`${props.host.instances.filter((item) => item.state === "active").length} active`} /><SummaryDatum label="Generation" value={String(props.host.generation)} detail={`${props.host.capabilities.length} capabilities`} /></section>
    <div class="grid gap-6 min-[1000px]:grid-cols-12">
      <section class="min-w-0 min-[1000px]:col-span-7"><SectionHeading title="Host information" meta="Authoritative snapshot" /><DefinitionRows rows={[["Kernel release", props.host.kernel.release], ["Architecture", props.host.topology.architecture], ["Logical CPUs observed", String(props.host.topology.cpus.length)], ["NUMA nodes", String(props.host.topology.numa_nodes.length)], ["Total memory", bytes(props.host.memory.total_bytes)], ["Host reserved", bytes(props.host.memory.host_reserved_bytes)]]} /></section>
      <section class="min-w-0 min-[1000px]:col-span-5"><SectionHeading title="Memory allocation" meta={`${assigned()}% assigned`} /><div class="border-t py-4"><Progress value={assigned()} /><div class="mt-3 flex justify-between text-xs text-muted-foreground"><span>{bytes(props.host.memory.assigned_bytes)} assigned</span><span>{bytes(props.host.memory.assignable_bytes)} pool</span></div></div><SectionHeading title="Capabilities" meta={`${props.host.capabilities.length} available`} /><div class="flex flex-wrap gap-2 border-t py-4"><For each={props.host.capabilities}>{(capability) => <Badge variant="secondary">{label(capability)}</Badge>}</For></div></section>
    </div>
    <section><SectionHeading title="Instances" meta={`${props.host.instances.length} registered`} /><InstanceTable instances={props.host.instances} /></section>
    <Show when={(props.host.diagnostics?.length ?? 0) > 0}><section><SectionHeading title="Diagnostics" meta={`${props.host.diagnostics?.length} findings`} /><Table><TableBody><For each={props.host.diagnostics}>{(item) => <TableRow><TableCell><StateBadge state={item.severity} /></TableCell><TableCell class="font-semibold">{item.code}</TableCell><TableCell class="whitespace-normal">{item.message}</TableCell></TableRow>}</For></TableBody></Table></section></Show>
  </div>
}

function HostMonitor(props: { host: HostSnapshot; operations: Operation[]; events: EventPage }) {
  return <div class="grid gap-6"><section><SectionHeading title="Tasks" meta={`${props.operations.length} retained`} /><OperationsTable operations={props.operations} /></section><section><SectionHeading title="Events" meta={`Sequence ${props.events.latest_sequence}`} /><Table><TableHeader><TableRow><TableHead>Sequence</TableHead><TableHead>Event</TableHead><TableHead>Resource</TableHead><TableHead>Generation</TableHead></TableRow></TableHeader><TableBody><For each={props.events.events} fallback={<EmptyRow columns={4} text="No host events" />}>{(event) => <TableRow><TableCell>{event.sequence}</TableCell><TableCell class="font-semibold">{label(event.kind)}</TableCell><TableCell>{event.resource ? `${label(event.resource.kind)} ${event.resource.id}` : "Host"}</TableCell><TableCell>{event.snapshot_generation}</TableCell></TableRow>}</For></TableBody></Table></section><section><SectionHeading title="Transactions" meta={`${props.host.transactions?.length ?? 0} retained`} /><Table><TableHeader><TableRow><TableHead>Transaction</TableHead><TableHead>State</TableHead><TableHead>Before</TableHead><TableHead>After</TableHead></TableRow></TableHeader><TableBody><For each={props.host.transactions} fallback={<EmptyRow columns={4} text="No resource transactions" />}>{(transaction) => <TableRow><TableCell class="font-semibold">{transaction.id}</TableCell><TableCell><StateBadge state={transaction.state} /></TableCell><TableCell>{transaction.generation_before ?? "—"}</TableCell><TableCell>{transaction.generation_after ?? "—"}</TableCell></TableRow>}</For></TableBody></Table></section></div>
}

function HostManage(props: { host: HostSnapshot }) {
  return <div class="grid gap-6"><section><SectionHeading title="CPU topology" meta={`${props.host.resource_pool.cpu_hardware_ids.length} delegated to pool`} /><Table><TableHeader><TableRow><TableHead>Hardware ID</TableHead><TableHead>Logical ID</TableHead><TableHead>Package</TableHead><TableHead>Core</TableHead><TableHead>Thread</TableHead><TableHead>NUMA</TableHead><TableHead>Online</TableHead></TableRow></TableHeader><TableBody><For each={props.host.topology.cpus} fallback={<EmptyRow columns={7} text="No control-kernel CPU topology reported" />}>{(cpu) => <TableRow><TableCell class="font-semibold">{cpu.hardware_id}</TableCell><TableCell>{cpu.logical_id}</TableCell><TableCell>{cpu.package_id}</TableCell><TableCell>{cpu.core_id}</TableCell><TableCell>{cpu.thread_index}</TableCell><TableCell>{cpu.numa_node}</TableCell><TableCell>{cpu.online ? "Yes" : "No"}</TableCell></TableRow>}</For></TableBody></Table></section><section><SectionHeading title="NUMA and memory" meta={`${props.host.topology.numa_nodes.length} nodes`} /><Table><TableHeader><TableRow><TableHead>Node</TableHead><TableHead>Logical CPUs</TableHead><TableHead>Total</TableHead><TableHead>Available</TableHead></TableRow></TableHeader><TableBody><For each={props.host.topology.numa_nodes}>{(node) => <TableRow><TableCell class="font-semibold">NUMA {node.id}</TableCell><TableCell>{node.logical_cpu_ids.join(", ") || "—"}</TableCell><TableCell>{bytes(node.total_memory_bytes)}</TableCell><TableCell>{bytes(node.available_memory_bytes)}</TableCell></TableRow>}</For></TableBody></Table></section><section><SectionHeading title="PCI devices" meta={`${props.host.resource_pool.devices?.length ?? 0} delegated`} /><Table><TableHeader><TableRow><TableHead>PCI address</TableHead><TableHead>Pool</TableHead><TableHead>IOMMU group</TableHead></TableRow></TableHeader><TableBody><For each={props.host.resource_pool.devices} fallback={<EmptyRow columns={3} text="No delegated PCI devices" />}>{(device) => <TableRow><TableCell class="font-semibold">{device.pci_id}</TableCell><TableCell>{device.pool_name}</TableCell><TableCell>{device.iommu_group ?? "—"}</TableCell></TableRow>}</For></TableBody></Table></section></div>
}

function InstancesView(props: { host: HostSnapshot; images: OsImage[] }) {
  const [open, setOpen] = createSignal(false)
  return <div class="min-h-full px-4 pt-4 pb-7 md:px-8 md:pt-6"><header class="flex items-start justify-between gap-4"><div><p class="text-xs text-muted-foreground">Inventory</p><h1 class="mt-1 text-[28px] font-[750]">Instances</h1><p class="mt-2 text-sm text-muted-foreground">{props.host.instances.length} peer-kernel instances on this host</p></div><Button size="sm" onClick={() => setOpen(true)}>New Instance</Button></header><section class="mt-5 border-y"><InstanceTable instances={props.host.instances} /></section><CreateInstanceDialog open={open()} onOpenChange={setOpen} host={props.host} images={props.images} /></div>
}

function InstanceRouteView(props: { host: HostSnapshot; images: ImageArtifact[]; credential: string; route: Extract<Route, { kind: "instance" }>; operations: Operation[]; busy?: number; onLifecycle: (instance: Instance, action: "start" | "stop" | "unload") => Promise<boolean>; onUpdate: (instance: Instance, input: { cpu_hardware_ids?: number[]; memory_bytes?: number; device_ids?: string[]; dry_run: boolean }) => Promise<boolean>; onLoad: (instance: Instance, input: { kernel_id: string; initrd_id?: string; command_line?: string }) => Promise<boolean>; onDelete: (instance: Instance) => Promise<boolean> }) {
  const instance = createMemo(() => props.host.instances.find((item) => item.id === props.route.id))
  return <Show when={instance()} fallback={<div class="p-8"><Alert variant="destructive"><CircleAlertIcon /><AlertTitle>Instance not found</AlertTitle><AlertDescription>The selected instance is no longer present in the authoritative host inventory.</AlertDescription></Alert></div>}>
    {(selected) => <ObjectView scope="Instances" title={selected().name} state={selected().state} tab={props.route.tab} base={`instances/${selected().id}`} actions={<InstanceActions instance={selected()} credential={props.credential} consoleAvailable={props.host.capabilities.includes("console")} busy={props.busy === selected().id} onLifecycle={props.onLifecycle} />}>
      <Switch><Match when={props.route.tab === "summary"}><InstanceSummary instance={selected()} /></Match><Match when={props.route.tab === "monitor"}><section><SectionHeading title="Instance tasks" meta="Retained operations" /><OperationsTable operations={props.operations.filter((operation) => operation.affected_resources?.some((item) => item.kind === "instance" && item.id === String(selected().id)))} /></section></Match><Match when={props.route.tab === "manage"}><InstanceManage instance={selected()} images={props.images} busy={props.busy === selected().id} onUpdate={props.onUpdate} onLoad={props.onLoad} onDelete={props.onDelete} /></Match></Switch>
    </ObjectView>}
  </Show>
}

function InstanceActions(props: { instance: Instance; credential: string; consoleAvailable: boolean; busy: boolean; onLifecycle: (instance: Instance, action: "start" | "stop" | "unload") => Promise<boolean> }) {
  const actions = () => availableActions(props.instance.state)
  const [consoleOpen, setConsoleOpen] = createSignal(false)
  return <><Show when={props.instance.state === "active" && props.consoleAvailable}><Button variant="outline" size="sm" disabled={props.busy} onClick={() => setConsoleOpen(true)}><SquareTerminalIcon aria-hidden="true" />Console</Button></Show><Show when={actions().includes("start")}><Button size="sm" disabled={props.busy} onClick={() => void props.onLifecycle(props.instance, "start")}><PlayIcon aria-hidden="true" />Start</Button></Show><Show when={actions().includes("stop")}><Button variant="secondary" size="sm" disabled={props.busy} onClick={() => void props.onLifecycle(props.instance, "stop")}><PowerIcon aria-hidden="true" />Stop</Button></Show><DropdownMenu><DropdownMenuTrigger class={buttonVariants({ variant: "outline", size: "sm" })}>Actions<ChevronDownIcon aria-hidden="true" /></DropdownMenuTrigger><DropdownMenuPortal><DropdownMenuContent><DropdownMenuItem disabled={!actions().includes("unload") || props.busy} onSelect={() => void props.onLifecycle(props.instance, "unload")}><SquareIcon aria-hidden="true" />Unload image</DropdownMenuItem><DropdownMenuItem onSelect={() => { window.location.hash = `#/instances/${props.instance.id}/manage` }}>Manage settings</DropdownMenuItem></DropdownMenuContent></DropdownMenuPortal></DropdownMenu><Show when={consoleOpen()}><InstanceConsole instanceId={props.instance.id} name={props.instance.name} token={props.credential} onClose={() => setConsoleOpen(false)} /></Show></>
}

function InstanceSummary(props: { instance: Instance }) {
  return <div class="grid gap-6"><section class="grid grid-cols-2 border-y min-[900px]:grid-cols-4"><SummaryDatum label="State" value={label(props.instance.state)} detail={`Generation ${props.instance.generation}`} /><SummaryDatum label="CPU allocation" value={`${props.instance.resources.cpu_hardware_ids.length} CPUs`} detail={props.instance.resources.cpu_hardware_ids.join(", ") || "None"} /><SummaryDatum label="Memory" value={bytes(props.instance.resources.memory_bytes)} detail={props.instance.resources.memory_base ? `Base ${props.instance.resources.memory_base}` : "No base reported"} /><SummaryDatum label="Kernel image" value={props.instance.image.present ? "Loaded" : "Not loaded"} detail={`Instance ${props.instance.id}`} /></section><div class="grid gap-6 min-[1000px]:grid-cols-12"><section class="min-[1000px]:col-span-7"><SectionHeading title="General information" meta="Authoritative state" /><DefinitionRows rows={[["Name", props.instance.name], ["Identifier", String(props.instance.id)], ["Lifecycle state", label(props.instance.state)], ["Generation", String(props.instance.generation)]]} /></section><section class="min-[1000px]:col-span-5"><SectionHeading title="Resource allocation" meta="Assigned to peer kernel" /><DefinitionRows rows={[["CPU hardware IDs", props.instance.resources.cpu_hardware_ids.join(", ") || "None"], ["Memory", bytes(props.instance.resources.memory_bytes)], ["Memory region", props.instance.resources.memory_region ?? "—"], ["Devices", props.instance.resources.device_ids?.join(", ") || "None"]]} /></section></div></div>
}

function InstanceManage(props: { instance: Instance; images: ImageArtifact[]; busy: boolean; onUpdate: (instance: Instance, input: { cpu_hardware_ids?: number[]; memory_bytes?: number; device_ids?: string[]; dry_run: boolean }) => Promise<boolean>; onLoad: (instance: Instance, input: { kernel_id: string; initrd_id?: string; command_line?: string }) => Promise<boolean>; onDelete: (instance: Instance) => Promise<boolean> }) {
  return <div class="grid gap-6"><ResourceEditor instance={props.instance} busy={props.busy} onSubmit={props.onUpdate} /><Show when={props.instance.state === "ready"}><ImageLoader instance={props.instance} images={props.images} busy={props.busy} onSubmit={props.onLoad} /></Show><section><SectionHeading title="Device assignment" meta={`${props.instance.resources.device_ids?.length ?? 0} devices`} /><Table><TableHeader><TableRow><TableHead>Device</TableHead><TableHead>Assignment</TableHead></TableRow></TableHeader><TableBody><For each={props.instance.resources.device_ids} fallback={<EmptyRow columns={2} text="No devices assigned" />}>{(device) => <TableRow><TableCell class="font-semibold">{device}</TableCell><TableCell>{props.instance.name}</TableCell></TableRow>}</For></TableBody></Table></section><DeleteInstance instance={props.instance} busy={props.busy} onDelete={props.onDelete} /></div>
}

function ImagesView(props: { images: OsImage[]; generation: number; busy: boolean; onUpload: (input: { expected_generation: number; file: File; label: string; architecture?: string; expected_sha256?: string }) => Promise<boolean> }) {
  const [open, setOpen] = createSignal(false)
  return <div class="min-h-full px-4 pt-4 pb-7 md:px-8 md:pt-6"><header class="flex items-start justify-between gap-4"><div><p class="text-xs text-muted-foreground">Inventory</p><h1 class="mt-1 text-[28px] font-[750]">Images</h1><p class="mt-2 text-sm text-muted-foreground">{props.images.length} operating system {props.images.length === 1 ? "image" : "images"}</p></div><Button size="sm" onClick={() => setOpen(true)}>Upload Image</Button></header><Alert class="mt-5"><CircleAlertIcon /><AlertTitle>Image provisioning is not connected yet</AlertTitle><AlertDescription>Uploaded images are validated and stored in the host inventory. Creating an instance from one is a separate capability still under development.</AlertDescription></Alert><section class="mt-5 border-y"><Table><TableHeader><TableRow><TableHead>Name</TableHead><TableHead>Format</TableHead><TableHead>Stored size</TableHead><TableHead>Virtual size</TableHead><TableHead>Architecture</TableHead><TableHead>Content identifier</TableHead></TableRow></TableHeader><TableBody><For each={props.images} fallback={<EmptyRow columns={6} text="No operating system images" />}>{(image) => <TableRow><TableCell class="font-semibold">{image.label}</TableCell><TableCell><Badge variant="secondary">{image.format.toUpperCase()}</Badge></TableCell><TableCell>{bytes(image.stored_bytes)}</TableCell><TableCell>{bytes(image.virtual_bytes)}</TableCell><TableCell>{image.architecture ?? "—"}</TableCell><TableCell class="max-w-[300px] truncate font-mono text-xs" title={image.id}>{image.id}</TableCell></TableRow>}</For></TableBody></Table></section><UploadOsImageDialog open={open()} onOpenChange={setOpen} busy={props.busy} generation={props.generation} onSubmit={props.onUpload} /></div>
}

function InstanceTable(props: { instances: Instance[] }) {
  return <Table><TableHeader><TableRow><TableHead>Instance</TableHead><TableHead>State</TableHead><TableHead>CPUs</TableHead><TableHead>Memory</TableHead><TableHead>Image</TableHead><TableHead>Generation</TableHead></TableRow></TableHeader><TableBody><For each={props.instances} fallback={<EmptyRow columns={6} text="No peer-kernel instances" />}>{(instance) => <TableRow><TableCell class="font-semibold"><a class="text-primary hover:underline" href={`#/instances/${instance.id}/summary`}>{instance.name}</a><span class="block text-xs font-normal text-muted-foreground">Instance {instance.id}</span></TableCell><TableCell><StateBadge state={instance.state} /></TableCell><TableCell>{instance.resources.cpu_hardware_ids.join(", ") || "—"}</TableCell><TableCell>{bytes(instance.resources.memory_bytes)}</TableCell><TableCell>{instance.image.present ? "Present" : "Not loaded"}</TableCell><TableCell>{instance.generation}</TableCell></TableRow>}</For></TableBody></Table>
}

function OperationsTable(props: { operations: Operation[] }) {
  return <Table><TableHeader><TableRow><TableHead>Task</TableHead><TableHead>Target</TableHead><TableHead>State</TableHead><TableHead>Progress</TableHead><TableHead>Started</TableHead><TableHead>Completed</TableHead></TableRow></TableHeader><TableBody><For each={props.operations} fallback={<EmptyRow columns={6} text="No retained operations" />}>{(operation) => <TableRow><TableCell class="font-semibold">{label(operation.kind)}</TableCell><TableCell>{operation.affected_resources?.map((item) => `${label(item.kind)} ${item.id}`).join(", ") || "Host"}</TableCell><TableCell><StateBadge state={operation.state} /></TableCell><TableCell>{operation.progress_percent === undefined ? "—" : `${operation.progress_percent}%`}</TableCell><TableCell>{timestamp(operation.created_at)}</TableCell><TableCell>{timestamp(operation.completed_at)}</TableCell></TableRow>}</For></TableBody></Table>
}

function CreateInstanceDialog(props: { open: boolean; onOpenChange: (open: boolean) => void; host: HostSnapshot; images: OsImage[] }) {
  const steps = ["OS Image", "Name", "Compute", "Review"] as const
  const [step, setStep] = createSignal(0)
  const [imageId, setImageId] = createSignal(props.images[0]?.id ?? "")
  const [name, setName] = createSignal("")
  const [cpuCount, setCpuCount] = createSignal("2")
  const [memory, setMemory] = createSignal("2048")
  const selectedImage = createMemo(() => props.images.find((image) => image.id === imageId()))
  const availableCpuCount = () => props.host.resource_pool.available_cpu_hardware_ids?.length ?? 0
  const currentValid = () => {
    if (step() === 0) return Boolean(selectedImage())
    if (step() === 1) return Boolean(name().trim())
    if (step() === 2) return Number.isInteger(Number(cpuCount())) && Number(cpuCount()) > 0 && Number(cpuCount()) <= availableCpuCount() && Number.isInteger(Number(memory())) && Number(memory()) > 0
    return true
  }
  const close = (open: boolean) => {
    props.onOpenChange(open)
    if (!open) setStep(0)
  }
  return <Dialog open={props.open} onOpenChange={close}><DialogContent class="h-[min(760px,calc(100svh-2rem))] max-w-[1100px] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden overscroll-contain p-0 sm:max-w-[1100px]" showCloseButton={false}><DialogHeader class="border-b px-7 py-5"><DialogTitle class="text-xl">Create New Instance</DialogTitle><DialogDescription>Configure an operating system and resources for a new peer-kernel instance.</DialogDescription></DialogHeader><div class="grid min-h-0 grid-cols-[220px_minmax(0,1fr)]"><nav class="border-r bg-muted/30 p-4" aria-label="Creation steps"><p class="px-3 pb-3 text-xs font-semibold tracking-wide text-muted-foreground">CONFIGURATION</p><div class="grid gap-1"><For each={steps}>{(item, index) => <Button type="button" variant={step() === index() ? "secondary" : "ghost"} class="h-auto justify-start gap-3 px-3 py-2.5 text-left" onClick={() => { if (index() <= step()) setStep(index()) }}><span class={`grid size-6 shrink-0 place-items-center rounded-full border text-xs ${step() === index() ? "border-primary bg-primary text-primary-foreground" : "border-border bg-background"}`}>{index() + 1}</span><span>{item}</span></Button>}</For></div></nav><main class="min-h-0 overflow-y-auto p-7"><Switch><Match when={step() === 0}><section><h3 class="text-lg font-semibold">Select an Operating System Image</h3><p class="mt-1 text-sm text-muted-foreground">Choose an image already uploaded to this host.</p><div class="mt-6 border-y"><Table><TableHeader><TableRow><TableHead>Image</TableHead><TableHead>Format</TableHead><TableHead>Architecture</TableHead><TableHead>Virtual size</TableHead></TableRow></TableHeader><TableBody><For each={props.images} fallback={<EmptyRow columns={4} text="No operating system images. Upload one from Images first." />}>{(image) => <TableRow class={imageId() === image.id ? "bg-muted" : ""}><TableCell><Button type="button" variant="ghost" class="h-auto justify-start gap-3 px-0 font-semibold" onClick={() => setImageId(image.id)}><span aria-hidden="true" class={`size-3 rounded-full border ${imageId() === image.id ? "border-primary bg-primary ring-2 ring-primary/20" : "border-muted-foreground"}`} />{image.label}</Button></TableCell><TableCell>{image.format.toUpperCase()}</TableCell><TableCell>{image.architecture ?? "—"}</TableCell><TableCell>{bytes(image.virtual_bytes)}</TableCell></TableRow>}</For></TableBody></Table></div></section></Match><Match when={step() === 1}><section class="max-w-2xl"><h3 class="text-lg font-semibold">Name This Instance</h3><p class="mt-1 text-sm text-muted-foreground">Use a name that identifies the operating system or workload.</p><div class="mt-7"><Field label="Instance name" for="new-instance-name"><Input id="new-instance-name" name="instance-name" autocomplete="off" maxlength={128} value={name()} onInput={(event) => setName(event.currentTarget.value)} placeholder="ubuntu-production…" /></Field></div></section></Match><Match when={step() === 2}><section class="max-w-2xl"><h3 class="text-lg font-semibold">Allocate Compute Resources</h3><p class="mt-1 text-sm text-muted-foreground">Kernmux will choose valid hardware IDs from the available resource pool.</p><div class="mt-7 grid gap-5 sm:grid-cols-2"><Field label="CPU count" for="new-instance-cpu-count"><Input id="new-instance-cpu-count" name="cpu-count" type="number" min="1" max={availableCpuCount()} value={cpuCount()} onInput={(event) => setCpuCount(event.currentTarget.value)} /></Field><Field label="Memory (MiB)" for="new-instance-memory"><Input id="new-instance-memory" name="memory-mib" type="number" min="1" value={memory()} onInput={(event) => setMemory(event.currentTarget.value)} /></Field></div><p class="mt-4 text-xs text-muted-foreground">{availableCpuCount()} CPUs are currently available to new instances.</p></section></Match><Match when={step() === 3}><section><h3 class="text-lg font-semibold">Review Configuration</h3><p class="mt-1 text-sm text-muted-foreground">Confirm the requested operating system and resources.</p><div class="mt-6 border-y"><DefinitionRows rows={[["Operating system image", selectedImage()?.label ?? "Not selected"], ["Instance name", name().trim() || "Not entered"], ["CPU", `${cpuCount()} CPUs`], ["Memory", `${memory()} MiB`]]} /></div><Alert class="mt-6"><CircleAlertIcon /><AlertTitle>Instance Provisioning Is Not Connected</AlertTitle><AlertDescription>The host can store this OS image, but it cannot yet create a bootable instance from it. Creation is disabled until that operation is implemented.</AlertDescription></Alert></section></Match></Switch></main></div><DialogFooter class="border-t px-7 py-4"><Button type="button" variant="outline" onClick={() => close(false)}>Cancel</Button><span class="flex-1" /><Button type="button" variant="outline" disabled={step() === 0} onClick={() => setStep(Math.max(0, step() - 1))}>Back</Button><Show when={step() < steps.length - 1} fallback={<Button type="button" disabled title="Image provisioning is not connected">Create Instance</Button>}><Button type="button" disabled={!currentValid()} onClick={() => setStep(Math.min(steps.length - 1, step() + 1))}>Next</Button></Show></DialogFooter></DialogContent></Dialog>
}

function ResourceEditor(props: { instance: Instance; busy: boolean; onSubmit: (instance: Instance, input: { cpu_hardware_ids?: number[]; memory_bytes?: number; device_ids?: string[]; dry_run: boolean }) => Promise<boolean> }) {
  const [cpus, setCpus] = createSignal(props.instance.resources.cpu_hardware_ids.join(","))
  const [memory, setMemory] = createSignal(String(Math.round(props.instance.resources.memory_bytes / 1024 / 1024)))
  const editable = () => props.instance.state === "ready"
  const valid = () => parseList(cpus()).length > 0 && Number.isInteger(Number(memory())) && Number(memory()) > 0
  const request = (dryRun: boolean) => props.onSubmit(props.instance, { cpu_hardware_ids: parseList(cpus()), memory_bytes: Number(memory()) * 1024 * 1024, dry_run: dryRun })
  return <section><SectionHeading title="Resource allocation" meta={editable() ? "Ready for changes" : "Unload before editing"} /><form class="grid gap-4 border-t py-4 min-[760px]:grid-cols-[1fr_1fr_auto_auto] min-[760px]:items-end" onSubmit={(event) => { event.preventDefault(); void request(false) }}><Field label="CPU hardware IDs" for="edit-instance-cpus"><Input id="edit-instance-cpus" name="cpu-hardware-ids" autocomplete="off" value={cpus()} onInput={(event) => setCpus(event.currentTarget.value)} disabled={!editable()} required /></Field><Field label="Memory (MiB)" for="edit-instance-memory"><Input id="edit-instance-memory" name="memory-mib" type="number" min="1" value={memory()} onInput={(event) => setMemory(event.currentTarget.value)} disabled={!editable()} required /></Field><Button type="button" variant="outline" disabled={!editable() || !valid() || props.busy} onClick={() => void request(true)}>Validate</Button><Button type="submit" disabled={!editable() || !valid() || props.busy}>{props.busy ? "Applying…" : "Apply"}</Button></form></section>
}

function ImageLoader(props: { instance: Instance; images: ImageArtifact[]; busy: boolean; onSubmit: (instance: Instance, input: { kernel_id: string; initrd_id?: string; command_line?: string }) => Promise<boolean> }) {
  const kernels = () => props.images.filter((item) => item.kind === "kernel")
  const initrds = () => props.images.filter((item) => item.kind === "initrd")
  const [kernel, setKernel] = createSignal(kernels()[0]?.id ?? "")
  const [initrd, setInitrd] = createSignal("")
  const [commandLine, setCommandLine] = createSignal("")
  return <section><SectionHeading title="Kernel image" meta="Load verified artifacts" /><form class="grid gap-4 border-t py-4 min-[900px]:grid-cols-2" onSubmit={(event) => { event.preventDefault(); void props.onSubmit(props.instance, { kernel_id: kernel(), initrd_id: initrd() || undefined, command_line: commandLine().trim() || undefined }) }}><Field label="Kernel" for="load-kernel"><NativeSelect id="load-kernel" name="kernel" class="w-full" value={kernel()} onChange={(event) => setKernel(event.currentTarget.value)} required><For each={kernels()}>{(image) => <NativeSelectOption value={image.id}>{shortId(image.id)}</NativeSelectOption>}</For></NativeSelect></Field><Field label="Initrd" for="load-initrd"><NativeSelect id="load-initrd" name="initrd" class="w-full" value={initrd()} onChange={(event) => setInitrd(event.currentTarget.value)}><NativeSelectOption value="">None</NativeSelectOption><For each={initrds()}>{(image) => <NativeSelectOption value={image.id}>{shortId(image.id)}</NativeSelectOption>}</For></NativeSelect></Field><Field label="Kernel command line" for="load-command-line" class="min-[900px]:col-span-2"><Input id="load-command-line" name="command-line" autocomplete="off" value={commandLine()} onInput={(event) => setCommandLine(event.currentTarget.value)} placeholder="console=mktty0" /></Field><div class="min-[900px]:col-span-2 flex justify-end"><Button type="submit" disabled={props.busy || !kernel()}>{props.busy ? "Loading…" : "Load Image"}</Button></div></form></section>
}

function DeleteInstance(props: { instance: Instance; busy: boolean; onDelete: (instance: Instance) => Promise<boolean> }) {
  const [open, setOpen] = createSignal(false)
  const allowed = () => props.instance.state === "ready"
  return <section><SectionHeading title="Delete instance" meta={allowed() ? "Permanent action" : "Unload before deleting"} /><div class="flex items-center justify-between gap-4 border-t py-4"><p class="text-sm text-muted-foreground">Remove the instance definition and return its resources to the pool.</p><Button variant="destructive" size="sm" disabled={!allowed() || props.busy} onClick={() => setOpen(true)}>Delete Instance</Button></div><AlertDialog open={open()} onOpenChange={setOpen}><AlertDialogPortal><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Delete {props.instance.name}?</AlertDialogTitle><AlertDialogDescription>This permanently removes Instance {props.instance.id}. The operation is allowed only while the instance is Ready.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><Button variant="destructive" disabled={props.busy} onClick={() => void (async () => { if (await props.onDelete(props.instance)) setOpen(false) })()}>Delete Instance</Button></AlertDialogFooter></AlertDialogContent></AlertDialogPortal></AlertDialog></section>
}

function UploadOsImageDialog(props: { open: boolean; onOpenChange: (open: boolean) => void; busy: boolean; generation: number; onSubmit: (input: { expected_generation: number; file: File; label: string; architecture?: string; expected_sha256?: string }) => Promise<boolean> }) {
  const [file, setFile] = createSignal<File>()
  const [name, setName] = createSignal("")
  const [architecture, setArchitecture] = createSignal("")
  const [expected, setExpected] = createSignal("")
  async function submit(event: SubmitEvent) {
    event.preventDefault()
    const selected = file()
    if (!selected) return
    const succeeded = await props.onSubmit({ expected_generation: props.generation, file: selected, label: name().trim(), architecture: architecture().trim() || undefined, expected_sha256: expected().trim() || undefined })
    if (succeeded) props.onOpenChange(false)
  }
  return <AlertDialog open={props.open} onOpenChange={props.onOpenChange}><AlertDialogPortal><AlertDialogContent><form onSubmit={(event) => void submit(event)}><AlertDialogHeader><AlertDialogTitle>Upload operating system image</AlertDialogTitle><AlertDialogDescription>Select a raw or QCOW2 Linux Cloud Image obtained from its distribution. Kernmux verifies the image and stores it by content identifier.</AlertDialogDescription></AlertDialogHeader><div class="mt-5 grid gap-4"><Field label="Image file" for="upload-os-image-file"><Input id="upload-os-image-file" name="image-file" type="file" accept=".img,.raw,.qcow2,application/octet-stream" onChange={(event) => { const selected = event.currentTarget.files?.item(0) ?? undefined; setFile(selected); if (selected && !name()) setName(selected.name.replace(/\.(img|raw|qcow2)$/i, "")) }} required /></Field><Field label="Name" for="upload-os-image-name"><Input id="upload-os-image-name" name="image-name" autocomplete="off" maxlength={128} value={name()} onInput={(event) => setName(event.currentTarget.value)} placeholder="Ubuntu 24.04" required /></Field><Field label="Architecture (optional)" for="upload-os-image-architecture"><Input id="upload-os-image-architecture" name="architecture" autocomplete="off" value={architecture()} onInput={(event) => setArchitecture(event.currentTarget.value)} placeholder="x86_64" /></Field><Field label="Expected SHA-256 (optional)" for="upload-os-image-sha256"><Input id="upload-os-image-sha256" name="expected-sha256" autocomplete="off" spellcheck={false} pattern="(?:sha256:)?[0-9a-fA-F]{64}" value={expected()} onInput={(event) => setExpected(event.currentTarget.value)} placeholder="64 hexadecimal characters" /></Field></div><AlertDialogFooter class="mt-6"><AlertDialogCancel type="button">Cancel</AlertDialogCancel><Button type="submit" disabled={props.busy || !file() || !name().trim()}>{props.busy ? "Uploading…" : "Upload Image"}</Button></AlertDialogFooter></form></AlertDialogContent></AlertDialogPortal></AlertDialog>
}

function Field(props: { label: string; for: string; class?: string; children: JSX.Element }) { return <label class={`grid min-w-0 gap-2 text-sm font-semibold ${props.class ?? ""}`} for={props.for}>{props.label}{props.children}</label> }
function parseList(value: string): number[] {
  const parts = value.split(",").map((item) => item.trim())
  if (parts.length === 0 || parts.some((item) => !/^\d+$/.test(item))) return []
  return [...new Set(parts.map(Number))]
}
function shortId(value: string): string { return value.length > 28 ? `${value.slice(0, 20)}…${value.slice(-6)}` : value }

function SummaryDatum(props: { label: string; value: string; detail: string }) { return <div class="min-w-0 px-4 py-3.5 min-[900px]:border-r min-[900px]:last:border-r-0"><span class="block truncate text-xs text-muted-foreground">{props.label}</span><strong class="mt-1 block truncate text-sm">{props.value}</strong><small class="mt-0.5 block truncate text-xs text-muted-foreground">{props.detail}</small></div> }
function SectionHeading(props: { title: string; meta: string }) { return <div class="flex min-h-11 items-center justify-between gap-4 border-t"><h2 class="text-[15px] font-[750]">{props.title}</h2><span class="text-xs font-semibold text-muted-foreground">{props.meta}</span></div> }
function DefinitionRows(props: { rows: Array<[string, string]> }) { return <dl class="grid grid-cols-1 border-t min-[760px]:grid-cols-2"><For each={props.rows}>{([name, value]) => <div class="min-w-0 border-b px-3.5 py-3 min-[760px]:border-r min-[760px]:even:border-r-0"><dt class="text-xs text-muted-foreground">{name}</dt><dd class="mt-1.5 truncate text-[13px] font-semibold" title={value}>{value}</dd></div>}</For></dl> }
function EmptyRow(props: { columns: number; text: string }) { return <TableRow><TableCell colSpan={props.columns} class="h-20 text-center text-muted-foreground">{props.text}</TableCell></TableRow> }
