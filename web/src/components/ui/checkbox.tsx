import type { ComponentProps, ValidComponent } from "solid-js"
import { splitProps } from "solid-js"
import { Checkbox as CheckboxPrimitive } from "@kobalte/core/checkbox"
import CheckIcon from "lucide-solid/icons/check"

import { cx } from "@/lib/cva"

export type CheckboxProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof CheckboxPrimitive<T>
>

export const Checkbox = <T extends ValidComponent = "div">(
  props: CheckboxProps<T>,
) => {
  const [, rest] = splitProps(props as CheckboxProps, ["class", "children"])

  return (
    <CheckboxPrimitive data-slot="checkbox" {...rest}>
      {(state) => (
        <>
          <CheckboxPrimitive.Input />
          <CheckboxPrimitive.Control
            data-slot="checkbox-control"
            class={cx(
              "peer inline-flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-border bg-card transition-[background-color,border-color,box-shadow] outline-none hover:border-input focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/35 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-[checked]:border-primary data-[checked]:bg-primary data-[checked]:text-primary-foreground dark:bg-input/30 dark:aria-invalid:ring-destructive/40 dark:data-[checked]:bg-primary",
              props.class,
            )}
          >
            <CheckboxPrimitive.Indicator
              data-slot="checkbox-indicator"
              class="grid place-content-center text-current transition-none [&>svg]:size-3.5"
            >
              <CheckIcon />
            </CheckboxPrimitive.Indicator>
          </CheckboxPrimitive.Control>
          {typeof props.children === "function"
            ? props.children(state)
            : props.children}
        </>
      )}
    </CheckboxPrimitive>
  )
}

export type CheckboxLabelProps<T extends ValidComponent = "label"> =
  ComponentProps<typeof CheckboxPrimitive.Label<T>>

export const CheckboxLabel = <T extends ValidComponent = "label">(
  props: CheckboxLabelProps<T>,
) => {
  const [, rest] = splitProps(props as CheckboxLabelProps, ["class"])

  return (
    <CheckboxPrimitive.Label
      data-slot="checkbox-label"
      class={cx(
        "flex items-center gap-2 text-sm leading-none font-medium select-none data-[disabled]:pointer-events-none data-[disabled]:cursor-not-allowed data-[disabled]:opacity-50",
        "data-[invalid]:text-destructive",
        props.class,
      )}
      {...rest}
    />
  )
}

export type CheckboxDescriptionProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof CheckboxPrimitive.Description<T>>

export const CheckboxDescription = <T extends ValidComponent = "div">(
  props: CheckboxDescriptionProps<T>,
) => {
  const [, rest] = splitProps(props as CheckboxDescriptionProps, ["class"])

  return (
    <CheckboxPrimitive.Description
      data-slot="checkbox-description"
      class={cx(
        "text-muted-foreground text-sm data-[disabled]:opacity-50",
        props.class,
      )}
      {...rest}
    />
  )
}

export type CheckboxInputProps<T extends ValidComponent = "input"> =
  ComponentProps<typeof CheckboxPrimitive.Input<T>>

export const CheckboxInput = <T extends ValidComponent = "input">(
  props: CheckboxInputProps<T>,
) => {
  const [, rest] = splitProps(props as CheckboxInputProps, ["class"])

  return (
    <CheckboxPrimitive.Input
      data-slot="checkbox-input"
      class={cx(
        "[&:focus-visible+div]:ring-ring/50 peer [&:focus-visible+div]:ring-[3px]",
        props.class,
      )}
      {...rest}
    />
  )
}

export type CheckboxControlProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof CheckboxPrimitive.Control<T>>

export const CheckboxControl = <T extends ValidComponent = "div">(
  props: CheckboxControlProps<T>,
) => {
  const [, rest] = splitProps(props as CheckboxControlProps, ["class"])

  return (
    <CheckboxPrimitive.Control
      data-slot="checkbox-control"
      class={cx(
        "peer-focus-visible:border-ring border-border bg-card transition-[background-color,border-color,box-shadow] hover:border-input dark:bg-input/30 data-[checked]:bg-primary data-[checked]:text-primary-foreground dark:data-[checked]:bg-primary data-[checked]:border-primary data-invalid:ring-destructive/20 dark:data-invalid:ring-destructive/40 data-invalid:border-destructive size-4 shrink-0 rounded-[4px] border outline-none data-disabled:cursor-not-allowed data-disabled:opacity-50",
        props.class,
      )}
      {...rest}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        class="flex items-center justify-center text-current transition-none"
      >
        <CheckIcon class="size-3.5" />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Control>
  )
}
