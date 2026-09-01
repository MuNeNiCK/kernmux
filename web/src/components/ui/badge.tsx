import type { ComponentProps, ValidComponent } from "solid-js"
import { splitProps } from "solid-js"
import { Badge as BadgePrimitive } from "@kobalte/core/badge"
import type { VariantProps } from "cva"

import { cva } from "@/lib/cva"

export const badgeVariants = cva({
  base: "inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-md border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-[color,box-shadow] focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/35 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3",
  variants: {
    variant: {
      default:
        "border-transparent bg-primary text-primary-foreground [a&]:hover:bg-primary/90",
      secondary:
        "border-border bg-card text-secondary-foreground [a&]:hover:border-input [a&]:hover:bg-accent",
      destructive:
        "border-transparent bg-destructive text-white [a&]:hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60",
      outline:
        "border-border bg-card text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
      ghost: "[a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
      link: "text-primary underline-offset-4 [a&]:hover:underline",
    },
  },
  defaultVariants: {
    variant: "default",
  },
})

export type BadgeProps<T extends ValidComponent = "span"> = ComponentProps<
  typeof BadgePrimitive<T>
> &
  VariantProps<typeof badgeVariants>

export const Badge = <T extends ValidComponent = "span">(
  props: BadgeProps<T>,
) => {
  const [, rest] = splitProps(props as BadgeProps, ["class", "variant"])

  return (
    <BadgePrimitive
      data-slot="badge"
      class={badgeVariants({
        variant: props.variant,
        class: props.class,
      })}
      {...rest}
    />
  )
}
