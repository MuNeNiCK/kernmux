import { splitProps, type ComponentProps, type ValidComponent } from "solid-js"
import { Progress as ProgressPrimitive } from "@kobalte/core/progress"

import { cx } from "@/lib/cva"

export type ProgressProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof ProgressPrimitive<T>
>

export const Progress = <T extends ValidComponent = "div">(
  props: ProgressProps<T>,
) => {
  const [, rest] = splitProps(props as ProgressProps, ["class", "children"])

  return (
    <ProgressPrimitive
      data-slot="progress"
      class={cx(
        "relative h-2 w-full overflow-hidden rounded-full bg-border/70",
        props.class,
      )}
      {...rest}
    >
      {props.children}
      <ProgressPrimitive.Track
        data-slot="progress-track"
        class="relative flex h-full w-full items-center overflow-x-hidden rounded-full bg-border/70"
      >
        <ProgressPrimitive.Fill
          data-slot="progress-indicator"
          class="h-full w-(--kb-progress-fill-width) bg-primary transition-all"
        />
      </ProgressPrimitive.Track>
    </ProgressPrimitive>
  )
}

export type ProgressTrackProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof ProgressPrimitive.Track<T>>

export const ProgressTrack = <T extends ValidComponent = "div">(
  props: ProgressTrackProps<T>,
) => {
  const [, rest] = splitProps(props as ProgressTrackProps, ["class"])

  return (
    <ProgressPrimitive.Track
      data-slot="progress-track"
      class={cx(
        "relative flex h-full w-full items-center overflow-x-hidden rounded-full bg-border/70",
        props.class,
      )}
      {...rest}
    />
  )
}

export type ProgressIndicatorProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof ProgressPrimitive.Fill<T>>

export const ProgressIndicator = <T extends ValidComponent = "div">(
  props: ProgressIndicatorProps<T>,
) => {
  const [, rest] = splitProps(props as ProgressIndicatorProps, ["class"])

  return (
    <ProgressPrimitive.Fill
      data-slot="progress-indicator"
      class={cx(
        "h-full w-(--kb-progress-fill-width) bg-primary transition-all",
        props.class,
      )}
      {...rest}
    />
  )
}

export type ProgressGroupProps = ComponentProps<"div">

export const ProgressGroup = (props: ProgressGroupProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="progress-group"
      class={cx("flex justify-between", props.class)}
      {...rest}
    />
  )
}

export type ProgressLabelProps<T extends ValidComponent = "span"> =
  ComponentProps<typeof ProgressPrimitive.Label<T>>

export const ProgressLabel = <T extends ValidComponent = "span">(
  props: ProgressLabelProps<T>,
) => {
  const [, rest] = splitProps(props as ProgressLabelProps, ["class"])

  return (
    <ProgressPrimitive.Label
      data-slot="progress-label"
      class={cx("text-sm font-medium", props.class)}
      {...rest}
    />
  )
}

export type ProgressValueLabelProps<T extends ValidComponent = "span"> =
  ComponentProps<typeof ProgressPrimitive.ValueLabel<T>>

export const ProgressValueLabel = <T extends ValidComponent = "span">(
  props: ProgressValueLabelProps<T>,
) => {
  const [, rest] = splitProps(props as ProgressValueLabelProps, ["class"])

  return (
    <ProgressPrimitive.ValueLabel
      data-slot="progress-value"
      class={cx("ml-auto text-sm text-muted-foreground tabular-nums", props.class)}
      {...rest}
    />
  )
}

export const ProgressValue = ProgressValueLabel
