import type { ComponentProps, ValidComponent } from "solid-js"
import { Show, mergeProps, splitProps } from "solid-js"
import { Dialog as DialogPrimitive } from "@kobalte/core/dialog"
import XIcon from "lucide-solid/icons/x"

import { cx } from "@/lib/cva"
import { buttonVariants } from "@/components/ui/button"

export const DialogPortal = DialogPrimitive.Portal

export type DialogProps = ComponentProps<typeof DialogPrimitive>

export const Dialog = (props: DialogProps) => {
  return <DialogPrimitive data-slot="dialog" {...props} />
}

export type DialogTriggerProps<T extends ValidComponent = "button"> =
  ComponentProps<typeof DialogPrimitive.Trigger<T>>

export const DialogTrigger = <T extends ValidComponent = "button">(
  props: DialogTriggerProps<T>,
) => {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

export type DialogCloseButtonProps<T extends ValidComponent = "button"> =
  ComponentProps<typeof DialogPrimitive.CloseButton<T>>

export const DialogCloseButton = <T extends ValidComponent = "button">(
  props: DialogCloseButtonProps<T>,
) => {
  return <DialogPrimitive.CloseButton data-slot="dialog-close" {...props} />
}

export type DialogContentProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof DialogPrimitive.Content<T>> & {
    showCloseButton?: boolean
  }

export const DialogContent = <T extends ValidComponent = "div">(
  props: DialogContentProps<T>,
) => {
  const merge = mergeProps(
    {
      showCloseButton: true,
    } as DialogContentProps,
    props,
  )
  const [, rest] = splitProps(merge, ["class", "children", "showCloseButton"])

  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay
        data-slot="dialog-overlay"
        class="fixed inset-0 z-50 bg-black/35 data-[closed]:animate-out data-[closed]:fade-out-0 data-[expanded]:animate-in data-[expanded]:fade-in-0"
      />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        class={cx(
          "fixed top-[50%] left-[50%] z-50 grid w-full max-w-[calc(100%-2rem)] translate-x-[-50%] translate-y-[-50%] gap-4 rounded-lg border bg-card p-6 text-card-foreground shadow-md duration-200 outline-none data-[closed]:animate-out data-[closed]:fade-out-0 data-[closed]:zoom-out-95 data-[expanded]:animate-in data-[expanded]:fade-in-0 data-[expanded]:zoom-in-95 sm:max-w-lg",
          props.class,
        )}
        {...rest}
      >
        {props.children}
        <Show when={props.showCloseButton}>
          <DialogPrimitive.CloseButton
            aria-label="Close"
            class={buttonVariants({
              variant: "ghost",
              size: "icon-sm",
              class:
                "absolute top-4 right-4 border-transparent text-muted-foreground hover:border-border hover:bg-primary/10 hover:text-foreground data-[expanded]:bg-primary/10 data-[expanded]:text-foreground",
            })}
          >
            <XIcon />
          </DialogPrimitive.CloseButton>
        </Show>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  )
}

export type DialogHeaderProps = ComponentProps<"div">

export const DialogHeader = (props: DialogHeaderProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="dialog-header"
      class={cx("flex flex-col gap-1.5 text-center sm:text-left", props.class)}
      {...rest}
    />
  )
}

export type DialogFooterProps = ComponentProps<"div">

export const DialogFooter = (props: DialogFooterProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="dialog-footer"
      class={cx(
        "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
        props.class,
      )}
      {...rest}
    />
  )
}

export type DialogTitleProps<T extends ValidComponent = "h2"> = ComponentProps<
  typeof DialogPrimitive.Title<T>
>

export const DialogTitle = <T extends ValidComponent = "h2">(
  props: DialogTitleProps<T>,
) => {
  const [, rest] = splitProps(props as DialogTitleProps, ["class"])

  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      class={cx("text-base leading-none font-semibold", props.class)}
      {...rest}
    />
  )
}

export type DialogDescriptionProps<T extends ValidComponent = "p"> =
  ComponentProps<typeof DialogPrimitive.Description<T>>

export const DialogDescription = <T extends ValidComponent = "p">(
  props: DialogDescriptionProps<T>,
) => {
  const [, rest] = splitProps(props as DialogDescriptionProps, ["class"])

  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      class={cx("text-muted-foreground text-sm", props.class)}
      {...rest}
    />
  )
}
