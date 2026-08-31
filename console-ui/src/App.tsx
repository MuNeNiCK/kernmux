import { For, Match, Show, Switch, createMemo, createSignal, onCleanup, onMount } from "solid-js"
import { Activity, Box, ChevronRight, CircleGauge, Cpu, HardDrive, Image, Layers3, RefreshCw, Server, Trash2 } from "lucide-solid"
import { ApiClient, type HostSnapshot, type ImageArtifact, type Instance, type Operation } from "./api"
import { consumeFragmentToken } from "./auth"
import { Badge } from "./components/ui/badge"
import { Button } from "./components/ui/button"
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogPortal, AlertDialogTitle } from "./components/ui/alert-dialog"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "./components/ui/table"
import { Tabs, TabsList, TabsTrigger } from "./components/ui/tabs"

type ObjectSelection = { kind: "host" } | { kind: "instance"; id: number } | { kind: "images" } | { kind: "operations" }
type Tab = "summary" | "monitor" | "manage"
const bytes = new Intl.NumberFormat(undefined, { style: "unit", unit: "gigabyte", unitDisplay: "short", maximumFractionDigits: 1 })
const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" })
const formatBytes = (value: number) => bytes.format(value / 2 ** 30)
const titleCase = (value: string) => value.replaceAll("_", " ").replace(/\b\w/g, letter => letter.toUpperCase())
const badgeVariant = (state: string) => ["healthy", "active", "succeeded", "applied", "loaded"].includes(state) ? "default" : ["failed", "indeterminate"].includes(state) ? "destructive" : "outline"

function initialNavigation(): { selection: ObjectSelection; tab: Tab } {
  const params = new URLSearchParams(window.location.search)
  const requestedTab = params.get("tab")
  const tab: Tab = requestedTab === "monitor" || requestedTab === "manage" ? requestedTab : "summary"
  const object = params.get("object") ?? "host"
  if (object === "images" || object === "operations") return { selection: { kind: object }, tab }
  if (object.startsWith("instance:")) {
    const id = Number(object.slice("instance:".length))
    if (Number.isInteger(id) && id >= 0) return { selection: { kind: "instance", id }, tab }
  }
  return { selection: { kind: "host" }, tab }
}

function parseCpuList(value: string): number[] {
  const result = value.split(",").flatMap(part => {
    const [start, end] = part.trim().split("-").map(Number)
    if (!Number.isInteger(start)) return []
    if (end === undefined) return [start!]
    if (!Number.isInteger(end) || start! > end) return []
    return Array.from({ length: end - start! + 1 }, (_, index) => start! + index)
  })
  if (!result.length || new Set(result).size !== result.length) return []
  return result
}

export function App() {
  const initial = initialNavigation()
  const [client, setClient] = createSignal<ApiClient>()
  const [host, setHost] = createSignal<HostSnapshot>()
  const [images, setImages] = createSignal<ImageArtifact[]>([])
  const [selection, setSelection] = createSignal<ObjectSelection>(initial.selection)
  const [tab, setTab] = createSignal<Tab>(initial.tab)
  const [loading, setLoading] = createSignal(true)
  const [busy, setBusy] = createSignal(false)
  const [error, setError] = createSignal<string>()
  const [confirmDelete, setConfirmDelete] = createSignal(false)
  const selectedInstance = createMemo(() => {
    const current = selection()
    return current.kind === "instance" ? host()?.instances.find(instance => instance.id === current.id) : undefined
  })


  async function refresh(silent = false) {
    const api = client()
    if (!api) return
    if (!silent) setLoading(true)
    try {
      const [nextHost, nextImages] = await Promise.all([api.result<HostSnapshot>("/api/1.0"), api.result<ImageArtifact[]>("/api/1.0/images")])
      setHost(nextHost); setImages(nextImages); setError(undefined)
    } catch (cause) { setError(cause instanceof Error ? cause.message : "The management gateway is unavailable.") }
    finally { setLoading(false) }
  }

  onMount(() => {
    try { setClient(new ApiClient(consumeFragmentToken())) }
    catch (cause) { setError(cause instanceof Error ? cause.message : "A management credential is required."); setLoading(false); return }
    void refresh()
    const timer = window.setInterval(() => void refresh(true), 5000)
    onCleanup(() => window.clearInterval(timer))
  })

  function navigate(next: ObjectSelection, nextTab: Tab = "summary") {
    setSelection(next); setTab(nextTab); setError(undefined)
    const object = next.kind === "instance" ? `instance:${next.id}` : next.kind
    window.history.replaceState(null, "", `/?object=${encodeURIComponent(object)}&tab=${nextTab}`)
  }

  async function mutate(method: string, path: string, body: unknown) {
    const api = client(); if (!api) return
    setBusy(true); setError(undefined)
    try { await api.mutate(method, path, body); await refresh(true) }
    catch (cause) { setError(cause instanceof Error ? cause.message : "The host action failed.") }
    finally { setBusy(false) }
  }

  const objectTitle = createMemo(() => {
    const current = selection()
    if (current.kind === "instance") return selectedInstance()?.name ?? `Instance ${current.id}`
    return current.kind === "host" ? "Host" : current.kind === "images" ? "Images" : "Operations"
  })

  return <div class="app-shell" data-testid="app-shell">
    <a class="skip-link" href="#main-content">Skip to Main Content</a>
    <header class="topbar"><div class="brand"><span class="brand-mark">K</span><strong>Kernmux</strong><span>Host Client</span></div><div class="top-status"><Badge variant={host()?.health === "healthy" ? "default" : "outline"}>{host() ? titleCase(host()!.health) : "Connecting"}</Badge><span>Local administration</span></div></header>
    <aside class="inventory" aria-label="Inventory">
      <div class="inventory-heading"><strong>Navigator</strong><span>Single host inventory</span></div>
      <nav>
        <button class="tree-row root" classList={{ selected: selection().kind === "host" }} data-testid="nav-host" onClick={() => navigate({ kind: "host" })}><Server aria-hidden="true" /><span>Host</span></button>
        <div class="tree-branch">
          <button class="tree-row" classList={{ selected: selection().kind === "host" && tab() === "manage" }} onClick={() => navigate({ kind: "host" }, "manage")}><CircleGauge aria-hidden="true" /><span>Manage</span></button>
          <button class="tree-row" classList={{ selected: selection().kind === "host" && tab() === "monitor" }} onClick={() => navigate({ kind: "host" }, "monitor")}><Activity aria-hidden="true" /><span>Monitor</span></button>
        </div>
        <div class="tree-section"><span>Instances</span><Badge>{host()?.instances.length ?? 0}</Badge></div>
        <div class="tree-branch">
          <For each={host()?.instances ?? []}>{instance => <button class="tree-row" classList={{ selected: selectedInstance()?.id === instance.id }} data-testid={`nav-instance-${instance.id}`} onClick={() => navigate({ kind: "instance", id: instance.id })}><Box aria-hidden="true" /><span class="truncate">{instance.name}</span><span class={`state-dot ${instance.state}`} aria-label={instance.state} /></button>}</For>
          <Show when={!host()?.instances.length}><span class="tree-empty">No instances</span></Show>
        </div>
        <button class="tree-row root" classList={{ selected: selection().kind === "images" }} onClick={() => navigate({ kind: "images" })}><Image aria-hidden="true" /><span>Images</span><Badge>{images().length}</Badge></button>
        <button class="tree-row root" classList={{ selected: selection().kind === "operations" }} onClick={() => navigate({ kind: "operations" })}><Layers3 aria-hidden="true" /><span>Operations</span></button>
      </nav>
    </aside>
    <main id="main-content" class="workspace" tabindex="-1">
      <section class="object-header">
        <div class="object-icon"><Switch><Match when={selection().kind === "host"}><Server /></Match><Match when={selection().kind === "instance"}><Box /></Match><Match when={selection().kind === "images"}><HardDrive /></Match><Match when={selection().kind === "operations"}><Activity /></Match></Switch></div>
        <div class="object-heading"><p>{selection().kind === "instance" ? "Virtual Kernel" : "Kernmux"}</p><h1>{objectTitle()}</h1></div>
        <div class="toolbar"><Button variant="outline" onClick={() => void refresh()} disabled={busy()}><RefreshCw aria-hidden="true" />Refresh</Button><InstanceToolbar instance={selectedInstance()} busy={busy()} mutate={mutate} /></div>
      </section>
      <Tabs value={tab()} onChange={value => navigate(selection(), value as Tab)}>
        <TabsList aria-label="Object views">
          <For each={["summary", "monitor", "manage"] as Tab[]}>{name => <TabsTrigger value={name} data-testid={`tab-${name}`}>{titleCase(name)}</TabsTrigger>}</For>
        </TabsList>
      </Tabs>
      <Show when={error()}><div class="alert" role="alert"><strong>Management request failed</strong><span>{error()} Try refreshing the host after checking the gateway service.</span></div></Show>
      <section class="content" aria-live="polite">
        <Show when={host()} fallback={<div class="loading">{loading() ? "Loading host inventory…" : "Host inventory is unavailable."}</div>}>
          <Switch>
            <Match when={selection().kind === "host"}><HostView host={host()} tab={tab()} busy={busy()} mutate={mutate} /></Match>
            <Match when={selection().kind === "instance"}><InstanceView instance={selectedInstance()} images={images()} tab={tab()} busy={busy()} mutate={mutate} onDelete={() => setConfirmDelete(true)} /></Match>
            <Match when={selection().kind === "images"}><ImagesView images={images()} generation={host()?.generation ?? 0} tab={tab()} busy={busy()} mutate={mutate} /></Match>
            <Match when={selection().kind === "operations"}><OperationsView operations={host()?.operations ?? []} /></Match>
          </Switch>
        </Show>
      </section>
    </main>
    <RecentTasks operations={host()?.operations ?? []} />
    <Show when={selectedInstance()}>{instance => <AlertDialog open={confirmDelete()} onOpenChange={setConfirmDelete}><AlertDialogPortal><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Delete {instance().name}?</AlertDialogTitle><AlertDialogDescription>This removes the kernel instance definition. This action cannot be undone.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction class="border-destructive bg-destructive text-destructive-foreground hover:bg-destructive/90" data-testid="confirm-delete" disabled={busy()} onClick={async () => { await mutate("DELETE", `/api/1.0/instances/${instance().id}`, { expected_generation: host()?.generation ?? instance().generation }); setConfirmDelete(false); navigate({ kind: "host" }) }}>Delete Instance</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialogPortal></AlertDialog>}</Show>
  </div>
}

function InstanceToolbar(props: { instance?: Instance; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void> }) {
  return <Show when={props.instance}>{instance => <><Show when={instance().state === "loaded"}><Button disabled={props.busy} onClick={() => props.mutate("POST", `/api/1.0/instances/${instance().id}/start`, { expected_generation: instance().generation })}>Start</Button></Show><Show when={instance().state === "active"}><Button variant="outline" disabled={props.busy} onClick={() => props.mutate("POST", `/api/1.0/instances/${instance().id}/stop`, { expected_generation: instance().generation, force: false })}>Stop</Button></Show></>}</Show>
}

function DefinitionTable(props: { rows: Array<[string, string | number]> }) { return <div class="panel"><Table><TableBody><For each={props.rows}>{row => <TableRow><TableHead scope="row">{row[0]}</TableHead><TableCell class="tabular">{row[1]}</TableCell></TableRow>}</For></TableBody></Table></div> }

function HostView(props: { host?: HostSnapshot; tab: Tab; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void> }) {
  return <Show when={props.host}>{host => <Switch>
    <Match when={props.tab === "summary"}><div class="view-grid"><section><h2>Host Information</h2><DefinitionTable rows={[["Status", titleCase(host().health)], ["Kernel", host().kernel.release], ["Architecture", host().topology.architecture], ["Generation", host().generation], ["Multikernel", host().kernel.multikernel_enabled ? "Enabled" : "Disabled"]]} /></section><section><h2>Capacity</h2><DefinitionTable rows={[["Logical CPUs", host().topology.cpus.length], ["Pool CPUs", host().resource_pool.cpu_hardware_ids.join(", ") || "None"], ["Memory", formatBytes(host().memory.total_bytes)], ["Assigned", formatBytes(host().memory.assigned_bytes)], ["Instances", host().instances.length]]} /></section><section class="span-two"><h2>Virtual Kernels</h2><InstanceTable instances={host().instances} /></section></div></Match>
    <Match when={props.tab === "monitor"}><div class="view-grid"><section class="span-two"><h2>CPU Topology</h2><div class="cpu-grid"><For each={host().topology.cpus}>{cpu => <div class="cpu-cell"><strong>CPU {cpu.logical_id}</strong><span>Core {cpu.core_id} · NUMA {cpu.numa_node}</span><Badge variant={cpu.online ? "default" : "destructive"}>{cpu.online ? "Online" : "Offline"}</Badge></div>}</For></div></section><section><h2>Memory</h2><DefinitionTable rows={[["Total", formatBytes(host().memory.total_bytes)], ["Host Reserved", formatBytes(host().memory.host_reserved_bytes)], ["Assignable", formatBytes(host().memory.assignable_bytes)], ["Assigned", formatBytes(host().memory.assigned_bytes)]]} /></section><section><h2>NUMA Nodes</h2><DefinitionTable rows={host().topology.numa_nodes.map(node => [`Node ${node.id}`, `${formatBytes(node.available_memory_bytes)} available`])} /></section></div></Match>
    <Match when={props.tab === "manage"}><HostManage host={host()} busy={props.busy} mutate={props.mutate} /></Match>
  </Switch>}</Show>
}

function InstanceTable(props: { instances: Instance[] }) { return <div class="panel"><Table><TableHeader><TableRow><TableHead>Name</TableHead><TableHead>State</TableHead><TableHead>CPUs</TableHead><TableHead>Memory</TableHead><TableHead>Image</TableHead></TableRow></TableHeader><TableBody><For each={props.instances}>{instance => <TableRow><TableCell class="font-medium">{instance.name}</TableCell><TableCell><Badge variant={badgeVariant(instance.state) as any}>{titleCase(instance.state)}</Badge></TableCell><TableCell class="tabular">{instance.resources.cpu_hardware_ids.join(", ")}</TableCell><TableCell class="tabular">{formatBytes(instance.resources.memory_bytes)}</TableCell><TableCell>{instance.image.present ? "Loaded" : "Not loaded"}</TableCell></TableRow>}</For><Show when={!props.instances.length}><TableRow><TableCell colSpan={5} class="empty-cell">No virtual kernels are defined.</TableCell></TableRow></Show></TableBody></Table></div> }

function HostManage(props: { host: HostSnapshot; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void> }) {
  let poolCpus!: HTMLInputElement, poolMemory!: HTMLInputElement, id!: HTMLInputElement, name!: HTMLInputElement, cpus!: HTMLInputElement, memory!: HTMLInputElement
  return <div class="view-grid"><form class="form-panel" onSubmit={event => { event.preventDefault(); void props.mutate("PUT", "/api/1.0/resource-pool", { expected_generation: props.host.generation, cpu_hardware_ids: parseCpuList(poolCpus.value), memory_bytes: Number(poolMemory.value) * 2 ** 30 }) }}><h2>Resource Pool</h2><Field label="CPU Hardware IDs" name="pool-cpus" ref={element => poolCpus = element} defaultValue={props.host.resource_pool.cpu_hardware_ids.join(",")} placeholder="2-7,10…" /><Field label="Memory (GiB)" name="pool-memory" ref={element => poolMemory = element} type="number" defaultValue={String(props.host.memory.assignable_bytes / 2 ** 30)} /><Button type="submit" disabled={props.busy}>Apply Resource Pool</Button></form><form class="form-panel" onSubmit={event => { event.preventDefault(); void props.mutate("POST", "/api/1.0/instances", { expected_generation: props.host.generation, id: Number(id.value), name: name.value, cpu_hardware_ids: parseCpuList(cpus.value), memory_bytes: Number(memory.value) * 2 ** 30 }) }}><h2>Create Virtual Kernel</h2><Field label="Instance ID" name="instance-id" ref={element => id = element} type="number" placeholder="3…" /><Field label="Name" name="instance-name" ref={element => name = element} placeholder="Build environment…" /><Field label="CPU Hardware IDs" name="instance-cpus" ref={element => cpus = element} placeholder="6-9…" /><Field label="Memory (GiB)" name="instance-memory" ref={element => memory = element} type="number" placeholder="8…" /><Button type="submit" disabled={props.busy}>Create Instance</Button></form></div>
}

function InstanceView(props: { instance?: Instance; images: ImageArtifact[]; tab: Tab; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void>; onDelete: () => void }) {
  return <Show when={props.instance} fallback={<div class="empty-state">This virtual kernel no longer exists. Select another object from the inventory.</div>}>{instance => <Switch>
    <Match when={props.tab === "summary"}><div class="view-grid"><section><h2>General</h2><DefinitionTable rows={[["Name", instance().name], ["ID", instance().id], ["State", titleCase(instance().state)], ["Generation", instance().generation], ["Kernel Image", instance().image.present ? "Loaded" : "Not loaded"]]} /></section><section><h2>Resources</h2><DefinitionTable rows={[["CPU Hardware IDs", instance().resources.cpu_hardware_ids.join(", ")], ["Memory", formatBytes(instance().resources.memory_bytes)], ["Devices", instance().resources.device_ids.join(", ") || "None"]]} /></section></div></Match>
    <Match when={props.tab === "monitor"}><div class="view-grid"><section><h2>Runtime Status</h2><DefinitionTable rows={[["Power State", titleCase(instance().state)], ["Allocated CPUs", instance().resources.cpu_hardware_ids.length], ["Allocated Memory", formatBytes(instance().resources.memory_bytes)]]} /></section><section><h2>CPU Allocation</h2><div class="cpu-grid"><For each={instance().resources.cpu_hardware_ids}>{cpu => <div class="cpu-cell"><strong>CPU {cpu}</strong><span>Assigned</span></div>}</For></div></section></div></Match>
    <Match when={props.tab === "manage"}><InstanceManage instance={instance()} images={props.images} busy={props.busy} mutate={props.mutate} onDelete={props.onDelete} /></Match>
  </Switch>}</Show>
}

function InstanceManage(props: { instance: Instance; images: ImageArtifact[]; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void>; onDelete: () => void }) {
  let kernel!: HTMLSelectElement, initrd!: HTMLSelectElement, commandLine!: HTMLInputElement
  const kernels = () => props.images.filter(image => image.kind === "kernel")
  const initrds = () => props.images.filter(image => image.kind === "initrd")
  return <div class="manage-stack"><section class="form-panel"><h2>Lifecycle</h2><p>Available actions reflect the authoritative kernel state.</p><div class="button-row"><Show when={props.instance.state === "loaded"}><Button disabled={props.busy} onClick={() => props.mutate("POST", `/api/1.0/instances/${props.instance.id}/start`, { expected_generation: props.instance.generation })}>Start Instance</Button></Show><Show when={props.instance.state === "active"}><Button variant="outline" disabled={props.busy} onClick={() => props.mutate("POST", `/api/1.0/instances/${props.instance.id}/stop`, { expected_generation: props.instance.generation, force: false })}>Stop Instance</Button></Show><Show when={props.instance.image.present && props.instance.state !== "active"}><Button variant="outline" disabled={props.busy} onClick={() => props.mutate("POST", `/api/1.0/instances/${props.instance.id}/unload`, { expected_generation: props.instance.generation })}>Unload Image</Button></Show><Button variant="destructive" data-testid="action-delete" disabled={props.busy || props.instance.state === "active"} title={props.instance.state === "active" ? "Stop the instance before deleting it." : undefined} onClick={props.onDelete}><Trash2 aria-hidden="true" />Delete Instance</Button></div></section><form class="form-panel" onSubmit={event => { event.preventDefault(); void props.mutate("POST", `/api/1.0/instances/${props.instance.id}/load-image`, { expected_generation: props.instance.generation, kernel_id: kernel.value, initrd_id: initrd.value || undefined, command_line: commandLine.value || undefined }) }}><h2>Load Managed Image</h2><label>Kernel Image<select ref={kernel} name="kernel-image" required><option value="">Select a kernel image</option><For each={kernels()}>{image => <option value={image.id}>{image.id}</option>}</For></select></label><label>Initrd Image<select ref={initrd} name="initrd-image"><option value="">None</option><For each={initrds()}>{image => <option value={image.id}>{image.id}</option>}</For></select></label><Field label="Kernel Command Line" name="command-line" ref={element => commandLine = element} placeholder="root=/dev/…" required={false} /><Button type="submit" disabled={props.busy || !kernels().length}>Load Image</Button><Show when={!kernels().length}><p class="form-help">Import a kernel image before loading this instance.</p></Show></form></div>
}

function ImagesView(props: { images: ImageArtifact[]; generation: number; tab: Tab; busy: boolean; mutate: (method: string, path: string, body: unknown) => Promise<void> }) {
  let kind!: HTMLSelectElement, source!: HTMLInputElement, expected!: HTMLInputElement
  return <div class="view-grid"><section class="span-two"><h2>Managed Images</h2><div class="panel"><Table><TableHeader><TableRow><TableHead>Artifact ID</TableHead><TableHead>Type</TableHead><TableHead>Size</TableHead><TableHead>Schema</TableHead></TableRow></TableHeader><TableBody><For each={props.images}>{image => <TableRow><TableCell class="mono break-all">{image.id}</TableCell><TableCell>{titleCase(image.kind)}</TableCell><TableCell>{formatBytes(image.bytes)}</TableCell><TableCell>{image.schema_version}</TableCell></TableRow>}</For><Show when={!props.images.length}><TableRow><TableCell colSpan={4} class="empty-cell">No managed images have been imported.</TableCell></TableRow></Show></TableBody></Table></div></section><Show when={props.tab === "manage"}><form class="form-panel span-two" onSubmit={event => { event.preventDefault(); void props.mutate("POST", "/api/1.0/images", { expected_generation: props.generation, kind: kind.value, source_path: source.value, expected_id: expected.value || undefined }) }}><h2>Import Image from Host</h2><label>Image Type<select ref={kind} name="image-kind"><option value="kernel">Kernel</option><option value="initrd">Initrd</option></select></label><Field label="Source Path" name="source-path" ref={element => source = element} placeholder="/var/lib/kernmux/import/vmlinuz…" /><Field label="Expected SHA-256 (Optional)" name="expected-id" ref={element => expected = element} placeholder="sha256…" required={false} /><Button type="submit" disabled={props.busy}>Import Image</Button></form></Show></div>
}

function OperationsView(props: { operations: Operation[] }) { return <section><h2>Host Operations</h2><div class="panel"><Table><TableHeader><TableRow><TableHead>Task</TableHead><TableHead>State</TableHead><TableHead>Progress</TableHead><TableHead>Started</TableHead><TableHead>Completed</TableHead></TableRow></TableHeader><TableBody><For each={props.operations}>{operation => <TableRow><TableCell>{titleCase(operation.kind)}</TableCell><TableCell><Badge variant={badgeVariant(operation.state) as any}>{titleCase(operation.state)}</Badge></TableCell><TableCell>{operation.progress_percent === undefined ? "—" : `${operation.progress_percent}%`}</TableCell><TableCell>{formatDate(operation.created_at)}</TableCell><TableCell>{operation.completed_at ? formatDate(operation.completed_at) : "—"}</TableCell></TableRow>}</For><Show when={!props.operations.length}><TableRow><TableCell colSpan={5} class="empty-cell">No host operations have been recorded.</TableCell></TableRow></Show></TableBody></Table></div></section> }

function RecentTasks(props: { operations: Operation[] }) { const recent = () => props.operations.slice(-4).reverse(); return <footer class="tasks"><div class="tasks-title"><strong>Recent Tasks</strong><span>{props.operations.length} tasks</span></div><div class="tasks-table"><Show when={recent().length} fallback={<span class="tasks-empty">No recent host tasks</span>}><For each={recent()}>{operation => <div class="task-row"><span>{titleCase(operation.kind)}</span><span>{operation.id}</span><span>{formatDate(operation.created_at)}</span><Badge variant={badgeVariant(operation.state) as any}>{titleCase(operation.state)}</Badge></div>}</For></Show></div></footer> }

function Field(props: { label: string; name: string; ref: (element: HTMLInputElement) => void; type?: string; defaultValue?: string; placeholder?: string; required?: boolean }) { return <label>{props.label}<input ref={element => { props.ref(element); if (props.defaultValue !== undefined) element.value = props.defaultValue }} name={props.name} type={props.type ?? "text"} placeholder={props.placeholder} autocomplete="off" required={props.required ?? true} /></label> }
function formatDate(value: string) { const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : dateTime.format(date) }
