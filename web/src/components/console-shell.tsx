import { For, Show, createEffect, createMemo, createSignal, type JSX } from "solid-js"
import BoxIcon from "lucide-solid/icons/box"
import BoxesIcon from "lucide-solid/icons/boxes"
import ChevronDownIcon from "lucide-solid/icons/chevron-down"
import ChevronUpIcon from "lucide-solid/icons/chevron-up"
import ImagesIcon from "lucide-solid/icons/images"
import MoonIcon from "lucide-solid/icons/moon"
import SearchIcon from "lucide-solid/icons/search"
import ServerIcon from "lucide-solid/icons/server"
import SunIcon from "lucide-solid/icons/sun"

import { Avatar, AvatarFallback } from "@/components/ui/avatar"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import {
  Sidebar, SidebarContent, SidebarGroup, SidebarGroupContent, SidebarGroupLabel,
  SidebarInset, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarProvider,
} from "@/components/ui/sidebar"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { label, timestamp } from "@/format"
import { cn } from "@/lib/utils"
import type { Instance, Operation } from "@/model"
import type { Route } from "@/route"

export function ConsoleShell(props: { route: Route; instances: Instance[]; operations: Operation[]; children: JSX.Element }) {
  const [isDark, setIsDark] = createSignal(false)
  const [query, setQuery] = createSignal("")
  const [tasksOpen, setTasksOpen] = createSignal(true)
  const filteredInstances = createMemo(() => {
    const value = query().trim().toLocaleLowerCase()
    return value ? props.instances.filter((instance) => instance.name.toLocaleLowerCase().includes(value)) : props.instances
  })
  createEffect(() => setIsDark(document.documentElement.classList.contains("dark")))

  function active(kind: Route["kind"], id?: number) {
    return props.route.kind === kind && (id === undefined || (props.route.kind === "instance" && props.route.id === id))
  }

  return (
    <SidebarProvider class="grid h-svh min-h-0 grid-rows-[52px_minmax(0,1fr)] overflow-hidden bg-background text-foreground md:grid-cols-[248px_minmax(0,1fr)]" style={{ "--sidebar-width": "248px" } as JSX.CSSProperties}>
      <a href="#main-content" class="sr-only z-50 bg-background px-3 py-2 text-foreground focus:not-sr-only">Skip to main content</a>
      <header class="col-span-full flex min-w-0 items-center gap-4 border-b border-console-header-foreground/15 bg-console-header px-4 text-console-header-foreground">
        <a href="#/host/summary" class="flex min-w-0 items-center gap-2.5 font-semibold focus-visible:outline-2 focus-visible:outline-offset-2">
          <span class="grid size-7 shrink-0 place-items-center rounded border border-current text-[11px] font-extrabold">KX</span>
          <span class="truncate">Kernmux</span>
          <span class="hidden text-xs font-normal text-console-header-foreground/65 sm:inline">Host Client</span>
        </a>
        <InputGroup class="ml-auto hidden w-[min(320px,30vw)] md:flex">
          <InputGroupAddon><SearchIcon aria-hidden="true" /></InputGroupAddon>
          <InputGroupInput name="instance-search" autocomplete="off" value={query()} onInput={(event) => setQuery(event.currentTarget.value)} placeholder="Search instances…" aria-label="Search instances" />
        </InputGroup>
        <Button variant="ghost" size="icon-sm" class="text-console-header-foreground hover:bg-console-header-foreground/10" aria-label={isDark() ? "Switch to light mode" : "Switch to dark mode"} onClick={() => {
          const next = !document.documentElement.classList.contains("dark")
          document.documentElement.classList.toggle("dark", next)
          setIsDark(next)
        }}>{isDark() ? <SunIcon aria-hidden="true" /> : <MoonIcon aria-hidden="true" />}</Button>
        <div class="hidden text-right sm:block"><div class="text-xs font-semibold">Local administration</div><div class="text-[11px] text-console-header-foreground/65">Bearer session</div></div>
        <Avatar size="sm"><AvatarFallback>LA</AvatarFallback></Avatar>
      </header>

      <Sidebar collapsible="none" class="max-md:hidden min-h-0 border-r md:flex">
        <SidebarContent class="px-3 py-4">
          <SidebarGroup class="p-0">
            <SidebarGroupLabel class="px-2 pb-2 text-[11px] font-bold tracking-[0.04em] uppercase">Navigator</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                <NavItem href="#/host/summary" icon={<ServerIcon />} label="Host" active={active("host")} />
                <li class="ml-4 border-l border-sidebar-border pl-2"><SidebarMenu class="gap-0.5">
                  <NavItem href="#/host/manage" label="Manage" active={props.route.kind === "host" && props.route.tab === "manage"} compact />
                  <NavItem href="#/host/monitor" label="Monitor" active={props.route.kind === "host" && props.route.tab === "monitor"} compact />
                </SidebarMenu></li>
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
          <SidebarGroup class="mt-5 p-0">
            <SidebarGroupLabel class="px-2 pb-2 text-[11px] font-bold tracking-[0.04em] uppercase">Inventory</SidebarGroupLabel>
            <SidebarGroupContent><SidebarMenu>
              <NavItem href="#/instances" icon={<BoxesIcon />} label={`Instances (${props.instances.length})`} active={active("instances")} />
              <Show when={filteredInstances().length > 0}><li class="ml-4 border-l border-sidebar-border pl-2"><SidebarMenu class="gap-0.5">
                <For each={filteredInstances()}>{(instance) => <NavItem href={`#/instances/${instance.id}/summary`} icon={<BoxIcon />} label={instance.name} active={active("instance", instance.id)} compact />}</For>
              </SidebarMenu></li></Show>
              <NavItem href="#/images" icon={<ImagesIcon />} label="Images" active={active("images")} />
            </SidebarMenu></SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
      </Sidebar>

      <SidebarInset class="grid min-h-0 min-w-0 grid-rows-[minmax(0,1fr)_auto] overflow-hidden md:col-start-2 md:row-start-2">
        <div id="main-content" class="min-h-0 min-w-0 overflow-auto" tabindex="-1">{props.children}</div>
        <section aria-label="Recent tasks" class="border-t bg-background">
          <div class="flex h-9 items-center px-4"><h2 class="text-xs font-bold">Recent Tasks</h2><span class="ml-auto text-xs text-muted-foreground">{props.operations.length} tasks</span><Button variant="ghost" size="xs" class="ml-2" onClick={() => setTasksOpen((value) => !value)}>{tasksOpen() ? <ChevronDownIcon aria-hidden="true" /> : <ChevronUpIcon aria-hidden="true" />}{tasksOpen() ? "Hide" : "Show"}</Button></div>
          <Show when={tasksOpen()}><div class="max-h-36 overflow-auto border-t"><Table>
            <TableHeader class="sticky top-0 bg-background"><TableRow><TableHead>Task</TableHead><TableHead>Target</TableHead><TableHead>Started</TableHead><TableHead>Result</TableHead></TableRow></TableHeader>
            <TableBody><For each={props.operations} fallback={<TableRow><TableCell colSpan={4} class="text-muted-foreground">No recent host tasks</TableCell></TableRow>}>
              {(operation) => <TableRow><TableCell class="font-semibold">{label(operation.kind)}</TableCell><TableCell>{operation.affected_resources?.map((item) => `${label(item.kind)} ${item.id}`).join(", ") || "Host"}</TableCell><TableCell>{timestamp(operation.created_at)}</TableCell><TableCell><StateBadge state={operation.state} /></TableCell></TableRow>}
            </For></TableBody>
          </Table></div></Show>
        </section>
      </SidebarInset>
    </SidebarProvider>
  )
}

function NavItem(props: { href: string; label: string; active: boolean; icon?: JSX.Element; compact?: boolean }) {
  return <SidebarMenuItem><SidebarMenuButton as="a" href={props.href} isActive={props.active} size={props.compact ? "sm" : "default"} class={cn("border-0 border-l-[3px] border-l-transparent", props.active && "border-l-sidebar-primary")}>{props.icon}<span>{props.label}</span></SidebarMenuButton></SidebarMenuItem>
}

export function StateBadge(props: { state: string }) {
  const color = () => props.state === "active" || props.state === "succeeded" || props.state === "healthy" || props.state === "applied" ? "text-success" : props.state === "failed" || props.state === "indeterminate" ? "text-destructive" : props.state === "loaded" || props.state === "running" ? "text-warning" : "text-info"
  return <Badge variant="ghost" class={`gap-1.5 px-0 font-semibold ${color()}`}><span class="size-1.5 rounded-full bg-current" />{label(props.state)}</Badge>
}
