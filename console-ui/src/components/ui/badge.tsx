import type { ComponentProps } from "solid-js"
import { splitProps } from "solid-js"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const badgeVariants = cva("inline-flex items-center rounded border px-2 py-0.5 text-xs font-medium", {
  variants: { variant: {
    default: "border-slate-300 bg-slate-100 text-slate-700",
    success: "border-emerald-300 bg-emerald-50 text-emerald-800",
    warning: "border-amber-300 bg-amber-50 text-amber-900",
    destructive: "border-red-300 bg-red-50 text-red-800",
  } }, defaultVariants: { variant: "default" },
})
export type BadgeProps = ComponentProps<"span"> & VariantProps<typeof badgeVariants>
export function Badge(props: BadgeProps) {
  const [local, rest] = splitProps(props, ["class", "variant"])
  return <span class={cn(badgeVariants({ variant: local.variant }), local.class)} {...rest} />
}
