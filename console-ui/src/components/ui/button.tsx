import type { ComponentProps } from "solid-js"
import { splitProps } from "solid-js"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"

const buttonVariants = cva(
  "inline-flex h-8 items-center justify-center gap-2 rounded border px-3 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-600 disabled:pointer-events-none disabled:opacity-50",
  { variants: { variant: {
    default: "border-sky-700 bg-sky-700 text-white hover:bg-sky-800",
    outline: "border-slate-300 bg-white text-slate-800 hover:bg-slate-100",
    ghost: "border-transparent bg-transparent text-slate-700 hover:bg-slate-100",
    destructive: "border-red-700 bg-red-700 text-white hover:bg-red-800",
  } }, defaultVariants: { variant: "default" } },
)

export type ButtonProps = ComponentProps<"button"> & VariantProps<typeof buttonVariants>
export function Button(props: ButtonProps) {
  const [local, rest] = splitProps(props, ["class", "variant"])
  return <button class={cn(buttonVariants({ variant: local.variant }), local.class)} {...rest} />
}
