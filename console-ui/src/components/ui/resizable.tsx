import {
  Show,
  mergeProps,
  splitProps,
  type ComponentProps,
  type ValidComponent,
} from "solid-js"
import ResizablePrimitive from "@corvu/resizable"
import GripVerticalIcon from "lucide-solid/icons/grip-vertical"

import { cx } from "@/lib/cva"

export type ResizableProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof ResizablePrimitive<T>
> & {
  direction?: "horizontal" | "vertical"
}

export const Resizable = <T extends ValidComponent>(
  props: ResizableProps<T>,
) => {
  const [, rest] = splitProps(props as ResizableProps, ["class", "direction"])

  return (
    <ResizablePrimitive
      data-slot="resizable"
      orientation={props.direction}
      class={cx("size-full", props.class)}
      {...rest}
    />
  )
}

export type ResizablePanelProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof ResizablePrimitive.Panel<T>> & {
    defaultSize?: number
  }

export const ResizablePanel = <T extends ValidComponent>(
  props: ResizablePanelProps<T>,
) => {
  const [, rest] = splitProps(props as ResizablePanelProps, [
    "defaultSize",
    "initialSize",
  ])
  const initialSize = () =>
    props.initialSize ?? (props.defaultSize == null ? undefined : props.defaultSize / 100)

  return (
    <ResizablePrimitive.Panel
      data-slot="resizable-panel"
      initialSize={initialSize()}
      {...rest}
    />
  )
}

export type ResizableHandleProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof ResizablePrimitive.Handle<T>> & {
    withHandle?: boolean
  }

export const ResizableHandle = <T extends ValidComponent>(
  props: ResizableHandleProps<T>,
) => {
  const merge = mergeProps({ withHandle: false } as ResizableHandleProps, props)
  const [, rest] = splitProps(merge, ["class", "withHandle"])

  return (
    <ResizablePrimitive.Handle
      data-slot="resizable-handle"
      class={cx(
        "relative flex w-px items-center justify-center bg-border after:absolute after:inset-y-0 after:left-1/2 after:w-1 after:-translate-x-1/2 focus-visible:ring-[3px] focus-visible:ring-ring/35 focus-visible:outline-hidden",
        "data-[orientation=vertical]:h-px data-[orientation=vertical]:w-full data-[orientation=vertical]:after:left-0 data-[orientation=vertical]:after:h-1 data-[orientation=vertical]:after:w-full data-[orientation=vertical]:after:translate-x-0 data-[orientation=vertical]:after:-translate-y-1/2 [&[data-orientation=vertical]>div]:rotate-90",
        props.class,
      )}
      {...rest}
    >
      <Show when={props.withHandle}>
        <div class="z-10 flex h-4 w-3 items-center justify-center rounded-md border bg-card text-muted-foreground">
          <GripVerticalIcon class="size-2.5" />
        </div>
      </Show>
    </ResizablePrimitive.Handle>
  )
}


export const ResizablePanelGroup = Resizable
