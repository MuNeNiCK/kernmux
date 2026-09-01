import type { ComponentProps, ValidComponent } from "solid-js"
import { Show, mergeProps, splitProps } from "solid-js"
import type { DynamicProps } from "@corvu/drawer"
import DrawerPrimitive from "@corvu/drawer"

import { cx } from "@/lib/cva"

export const DrawerPortal = DrawerPrimitive.Portal

export type DrawerProps = ComponentProps<typeof DrawerPrimitive>

export const Drawer = (props: DrawerProps) => {
  return <DrawerPrimitive data-slot="drawer" {...props} />
}

export type DrawerTriggerProps<T extends ValidComponent = "button"> =
  ComponentProps<typeof DrawerPrimitive.Trigger<T>>

export const DrawerTrigger = <T extends ValidComponent = "button">(
  props: DrawerTriggerProps<T>,
) => {
  return <DrawerPrimitive.Trigger data-slot="drawer-trigger" {...props} />
}

export type DrawerCloseProps<T extends ValidComponent = "button"> =
  ComponentProps<typeof DrawerPrimitive.Close<T>>

export const DrawerClose = <T extends ValidComponent = "button">(
  props: DrawerCloseProps<T>,
) => {
  return <DrawerPrimitive.Close data-slot="drawer-close" {...props} />
}

export type DrawerContentProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof DrawerPrimitive.Content<T>> & {
    withHandle?: boolean
  }

export const DrawerContent = <T extends ValidComponent = "div">(
  props: DrawerContentProps<T>,
) => {
  const context = DrawerPrimitive.useContext()
  const dialogContext = DrawerPrimitive.useDialogContext()

  const merge = mergeProps<DrawerContentProps[]>(
    {
      withHandle: context.side() === "bottom",
    },
    props as DrawerContentProps,
  )
  const [, rest] = splitProps(merge, ["class", "children", "withHandle"])

  return (
    <>
      <Show when={dialogContext.modal()}>
        <DrawerPrimitive.Overlay
          data-slot="drawer-overlay"
          class="fixed inset-0 z-50 bg-black/35 data-[transitioning]:transition-colors data-[transitioning]:duration-500 data-[transitioning]:ease-[cubic-bezier(0.32,0.72,0,1)]"
          style={{
            "background-color": `rgb(0 0 0 / ${0.35 * context.openPercentage()}`,
          }}
        />
      </Show>
      <DrawerPrimitive.Content
        data-slot="drawer-content"
        data-side={context.side()}
        class={cx(
          "group/drawer-content fixed z-50 flex h-auto flex-col bg-card text-card-foreground shadow-md outline-none after:absolute after:bg-inherit data-[transitioning]:transition-transform data-[transitioning]:duration-500 data-[transitioning]:ease-[cubic-bezier(0.32,0.72,0,1)]",
          context.side() === "bottom" && [
            "inset-x-0 bottom-0 mt-24 max-h-[80vh] rounded-t-lg border-t",
            "after:inset-x-0 after:top-[calc(100%-1px)] after:h-1/2",
          ],
          context.side() === "top" && [
            "inset-x-0 top-0 mb-24 max-h-[80vh] rounded-b-lg border-b",
            "after:inset-x-0 after:bottom-[calc(100%-1px)] after:h-1/2",
          ],
          context.side() === "left" && [
            "inset-y-0 left-0 w-3/4 border-r sm:max-w-sm",
            "after:inset-y-0 after:right-[calc(100%-1px)] after:w-1/2",
          ],
          context.side() === "right" && [
            "inset-y-0 right-0 w-3/4 border-l sm:max-w-sm",
            "after:inset-y-0 after:left-[calc(100%-1px)] after:w-1/2",
          ],
          props.class,
        )}
        {...rest}
      >
        <Show when={props.withHandle}>
          <div
            class={cx(
              "shrink-0 self-center rounded-full bg-border",
              context.side() === "bottom" && "mt-4 h-1.5 w-[88px]",
            )}
          />
        </Show>
        {props.children}
      </DrawerPrimitive.Content>
    </>
  )
}

export type DrawerLabelProps<T extends ValidComponent = "h2"> = ComponentProps<
  typeof DrawerPrimitive.Label<T>
>

export const DrawerLabel = <T extends ValidComponent = "h2">(
  props: DynamicProps<T, DrawerLabelProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerLabelProps, ["class"])

  return (
    <DrawerPrimitive.Label
      data-slot="drawer-label"
      class={cx("font-semibold text-foreground", props.class)}
      {...rest}
    />
  )
}

export type DrawerTitleProps<T extends ValidComponent = "h2"> = ComponentProps<
  typeof DrawerPrimitive.Label<T>
>

export const DrawerTitle = <T extends ValidComponent = "h2">(
  props: DynamicProps<T, DrawerTitleProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerTitleProps, ["class"])

  return (
    <DrawerPrimitive.Label
      data-slot="drawer-title"
      class={cx("font-semibold text-foreground", props.class)}
      {...rest}
    />
  )
}

export type DrawerDescriptionProps<T extends ValidComponent = "p"> =
  ComponentProps<typeof DrawerPrimitive.Description<T>>

export const DrawerDescription = <T extends ValidComponent = "p">(
  props: DynamicProps<T, DrawerDescriptionProps<T>>,
) => {
  const [, rest] = splitProps(props as DrawerDescriptionProps, ["class"])

  return (
    <DrawerPrimitive.Description
      data-slot="drawer-description"
      class={cx("text-muted-foreground text-sm", props.class)}
      {...rest}
    />
  )
}

export type DrawerHeaderProps = ComponentProps<"div">

export const DrawerHeader = (props: DrawerHeaderProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="drawer-header"
      class={cx(
        "flex flex-col gap-1.5 p-6 group-data-[side=bottom]/drawer-content:text-center group-data-[side=top]/drawer-content:text-center md:text-left",
        props.class,
      )}
      {...rest}
    />
  )
}

export type DrawerFooterProps = ComponentProps<"div">

export const DrawerFooter = (props: DrawerFooterProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="drawer-footer"
      class={cx("mt-auto flex flex-col gap-2 p-6", props.class)}
      {...rest}
    />
  )
}
