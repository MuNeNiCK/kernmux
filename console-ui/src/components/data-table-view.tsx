import { For, Show, createMemo, createSignal, type JSX } from "solid-js"
import Columns3CogIcon from "lucide-solid/icons/columns-3-cog"
import SearchIcon from "lucide-solid/icons/search"

import { buttonVariants } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import EmptyState from "@/components/empty-state"

export type ConsoleColumn = { id: string; label: string; optional?: boolean; class?: string }
export type ConsoleRow = { id: string; search: string; cells: Record<string, JSX.Element>; testId?: string }

export default function DataTableView(props: { columns: ConsoleColumn[]; rows: ConsoleRow[]; filterPlaceholder: string; emptyTitle: string; emptyDescription: string; emptyAction?: JSX.Element; toolbar?: JSX.Element; onRowClick?: (id: string) => void; testId?: string }) {
  const [query, setQuery] = createSignal("")
  const [visible, setVisible] = createSignal(props.columns.map(column => column.id))
  const rows = createMemo(() => { const value = query().trim().toLowerCase(); return value ? props.rows.filter(row => row.search.toLowerCase().includes(value)) : props.rows })
  const shownColumns = createMemo(() => props.columns.filter(column => visible().includes(column.id)))
  const optional = () => props.columns.filter(column => column.optional)
  const toggle = (id: string, checked: boolean) => setVisible(current => checked ? [...new Set([...current, id])] : current.filter(value => value !== id))

  return <section class="grid min-h-0 min-w-0 grid-rows-[48px_minmax(0,1fr)_auto] overflow-hidden border-y" data-testid={props.testId}>
    <div class="flex min-w-0 items-center gap-2 border-b">
      <InputGroup class="min-w-0 max-w-[320px] flex-1"><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput aria-label={props.filterPlaceholder} value={query()} onInput={event => setQuery(event.currentTarget.value)} placeholder={props.filterPlaceholder} /></InputGroup>
      <div class="ml-auto flex items-center gap-2">{props.toolbar}<Show when={optional().length}><DropdownMenu><DropdownMenuTrigger class={buttonVariants({ variant: "outline", size: "sm" })}><Columns3CogIcon />Columns</DropdownMenuTrigger><DropdownMenuContent><For each={optional()}>{column => <DropdownMenuCheckboxItem checked={visible().includes(column.id)} onChange={checked => toggle(column.id, checked)}>{column.label}</DropdownMenuCheckboxItem>}</For></DropdownMenuContent></DropdownMenu></Show></div>
    </div>
    <div class="min-h-0 min-w-0 overflow-auto"><Show when={rows().length} fallback={<EmptyState compact title={query() ? "No matching records" : props.emptyTitle} description={query() ? "Change or clear the current filter." : props.emptyDescription} action={query() ? undefined : props.emptyAction} />}>
      <Table class="min-w-[760px]"><TableHeader class="sticky top-0 z-10 bg-background"><TableRow><For each={shownColumns()}>{column => <TableHead class={column.class}>{column.label}</TableHead>}</For></TableRow></TableHeader><TableBody><For each={rows()}>{row => <TableRow data-testid={row.testId} class={props.onRowClick ? "cursor-pointer" : undefined} tabindex={props.onRowClick ? 0 : undefined} onClick={() => props.onRowClick?.(row.id)} onKeyDown={event => { if (props.onRowClick && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); props.onRowClick(row.id) } }}><For each={shownColumns()}>{column => <TableCell class={column.class}>{row.cells[column.id]}</TableCell>}</For></TableRow>}</For></TableBody></Table>
    </Show></div>
    <footer class="flex min-h-9 items-center justify-end px-3 text-xs text-muted-foreground">Showing {rows().length} of {props.rows.length}</footer>
  </section>
}
