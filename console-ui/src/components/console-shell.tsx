import { type JSX, createSignal } from "solid-js"
import MenuIcon from "lucide-solid/icons/menu"

import { Button } from "@/components/ui/button"
import { Drawer, DrawerContent, DrawerLabel } from "@/components/ui/drawer"
import { Sidebar, SidebarContent, SidebarInset, SidebarProvider } from "@/components/ui/sidebar"
import { cn } from "@/lib/utils"

export function ConsoleShell(props: { inventory: () => JSX.Element; children: JSX.Element; status?: JSX.Element; class?: string }) {
  const [mobileOpen, setMobileOpen] = createSignal(false)
  return <SidebarProvider data-testid="app-shell" class={cn("grid h-svh min-h-0 grid-rows-[56px_minmax(0,1fr)] overflow-hidden bg-background text-foreground md:grid-cols-[248px_minmax(0,1fr)]", props.class)} style={{ "--sidebar-width": "248px" } as JSX.CSSProperties}>
    <header class="col-span-full flex min-w-0 items-center gap-3 border-b border-console-header-foreground/15 bg-console-header px-4 text-console-header-foreground md:px-5">
      <Button type="button" variant="ghost" size="icon-sm" class="text-console-header-foreground hover:bg-console-header-foreground/10 md:hidden" aria-label="Open inventory" data-testid="mobile-inventory-trigger" onClick={() => setMobileOpen(true)}><MenuIcon /></Button>
      <div class="flex min-w-0 items-center gap-2.5 font-semibold"><span class="grid size-7 shrink-0 place-items-center rounded border border-current text-[11px] font-extrabold">K</span><span class="truncate">Kernmux</span><span class="hidden text-xs font-normal text-console-header-foreground/70 sm:inline">Host Client</span></div>
      <div class="ml-auto flex items-center gap-3">{props.status}</div>
    </header>
    <Sidebar collapsible="none" class="hidden min-h-0 border-r md:flex" aria-label="Inventory"><SidebarContent class="px-3 py-4">{props.inventory()}</SidebarContent></Sidebar>
    <Drawer open={mobileOpen()} onOpenChange={setMobileOpen} side="left"><DrawerContent data-testid="mobile-inventory" class="!w-[320px] max-w-[88vw]" withHandle={false}><DrawerLabel class="border-b px-4 py-4">Kernmux inventory</DrawerLabel><div class="min-h-0 flex-1 overflow-auto px-3 py-4" onClick={() => setMobileOpen(false)}>{props.inventory()}</div></DrawerContent></Drawer>
    <SidebarInset id="main-content" tabindex="-1" class="min-h-0 min-w-0 overflow-hidden outline-none md:col-start-2 md:row-start-2">{props.children}</SidebarInset>
  </SidebarProvider>
}
