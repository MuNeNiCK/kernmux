import type { ComponentProps } from "solid-js"
import { cn } from "@/lib/utils"
export function Separator(props: ComponentProps<"hr">) { return <hr {...props} class={cn("border-0 border-t border-slate-200", props.class)} /> }
