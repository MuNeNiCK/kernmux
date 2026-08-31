import type { ComponentProps } from "solid-js"
import { splitProps } from "solid-js"
import { cn } from "@/lib/utils"

export function Table(props: ComponentProps<"table">) { const [local, rest] = splitProps(props, ["class"]); return <table class={cn("w-full border-collapse text-sm", local.class)} {...rest} /> }
export function TableHeader(props: ComponentProps<"thead">) { const [local, rest] = splitProps(props, ["class"]); return <thead class={cn("border-b border-slate-300 bg-slate-50", local.class)} {...rest} /> }
export function TableBody(props: ComponentProps<"tbody">) { const [local, rest] = splitProps(props, ["class"]); return <tbody class={cn("divide-y divide-slate-200", local.class)} {...rest} /> }
export function TableRow(props: ComponentProps<"tr">) { const [local, rest] = splitProps(props, ["class"]); return <tr class={cn("hover:bg-slate-50", local.class)} {...rest} /> }
export function TableHead(props: ComponentProps<"th">) { const [local, rest] = splitProps(props, ["class"]); return <th class={cn("h-9 px-3 text-left text-xs font-semibold text-slate-600", local.class)} {...rest} /> }
export function TableCell(props: ComponentProps<"td">) { const [local, rest] = splitProps(props, ["class"]); return <td class={cn("px-3 py-2.5 align-middle text-slate-800", local.class)} {...rest} /> }
