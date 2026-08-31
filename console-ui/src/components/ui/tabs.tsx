import type { ComponentProps } from "solid-js"
import { splitProps } from "solid-js"
import { cn } from "@/lib/utils"

export function TabsList(props: ComponentProps<"div">) {
  const [local, rest] = splitProps(props, ["class"])
  return <div role="tablist" class={cn("flex h-10 border-b border-slate-300", local.class)} {...rest} />
}
export function TabsTrigger(props: ComponentProps<"button"> & { active?: boolean }) {
  const [local, rest] = splitProps(props, ["class", "active"])
  return <button role="tab" aria-selected={local.active} class={cn("relative px-4 text-sm font-medium text-slate-600 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-sky-600", local.active && "text-sky-800 after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:bg-sky-700", local.class)} {...rest} />
}
