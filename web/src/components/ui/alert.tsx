import type { ComponentProps, ValidComponent } from "solid-js"
import { splitProps } from "solid-js"
import { Alert as AlertPrimitive } from "@kobalte/core/alert"
import type { VariantProps } from "cva"

import { cva, cx } from "@/lib/cva"

export const alertVariants = cva({
  base: "relative grid w-full grid-cols-[0_1fr] items-start gap-y-0.5 rounded-lg border px-4 py-3 text-sm shadow-xs has-data-[slot=alert-action]:pr-18 has-[>svg]:grid-cols-[calc(var(--spacing)*4)_1fr] has-[>svg]:gap-x-3 [&>svg]:size-4 [&>svg]:translate-y-0.5 [&>svg]:text-current",
  variants: {
    variant: {
      default: "bg-card text-card-foreground",
      destructive:
        "bg-card text-destructive *:data-[slot=alert-description]:text-destructive/90 [&>svg]:text-current",
    },
  },
  defaultVariants: {
    variant: "default",
  },
})

export type AlertProps<T extends ValidComponent = "button"> = ComponentProps<
  typeof AlertPrimitive<T>
> &
  VariantProps<typeof alertVariants>

export const Alert = <T extends ValidComponent = "button">(
  props: AlertProps<T>,
) => {
  const [, rest] = splitProps(props as AlertProps, ["class", "variant"])

  return (
    <AlertPrimitive
      data-slot="alert"
      class={alertVariants({
        variant: props.variant,
        class: props.class,
      })}
      {...rest}
    />
  )
}

export type AlertTitleProps = ComponentProps<"div">

export const AlertTitle = (props: AlertTitleProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="alert-title"
      class={cx(
        "col-start-2 line-clamp-1 min-h-4 font-medium tracking-tight",
        props.class,
      )}
      {...rest}
    />
  )
}

export type AlertDescriptionProps = ComponentProps<"div">

export const AlertDescription = (props: AlertDescriptionProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="alert-description"
      class={cx(
        "text-muted-foreground col-start-2 grid justify-items-start gap-1 text-sm [&_p]:leading-relaxed",
        props.class,
      )}
      {...rest}
    />
  )
}

export type AlertActionProps = ComponentProps<"div">

export const AlertAction = (props: AlertActionProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="alert-action"
      class={cx("absolute top-2 right-2", props.class)}
      {...rest}
    />
  )
}
