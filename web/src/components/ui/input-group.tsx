import { splitProps, type JSX } from "solid-js"
import { cva, type VariantProps } from "class-variance-authority"

import { cn } from "@/lib/utils"
import { Button, type ButtonProps } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"

function InputGroup(props: JSX.HTMLAttributes<HTMLDivElement>) {
  const [local, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="input-group"
      role="group"
      class={cn(
        "group/input-group relative flex h-9 w-full min-w-0 items-center rounded-full border border-input bg-background transition-[border-color,box-shadow] outline-none has-[>textarea]:h-auto has-[>textarea]:rounded-md dark:bg-input/20",
        "has-[>[data-align=block-end]]:h-auto has-[>[data-align=block-end]]:flex-col has-[>[data-align=block-start]]:h-auto has-[>[data-align=block-start]]:flex-col",
        "has-[>[data-align=block-end]]:[&>input]:pt-3 has-[>[data-align=block-start]]:[&>input]:pb-3 has-[>[data-align=inline-end]]:[&>input]:pr-1.5 has-[>[data-align=inline-start]]:[&>input]:pl-1.5",
        "hover:border-ring/45",
        "has-[[data-slot=input-group-control]:focus-visible]:border-ring has-[[data-slot=input-group-control]:focus-visible]:ring-[2px] has-[[data-slot=input-group-control]:focus-visible]:ring-ring/25",
        "has-[[data-slot][aria-invalid=true]]:border-destructive has-[[data-slot][aria-invalid=true]]:ring-destructive/20 dark:has-[[data-slot][aria-invalid=true]]:ring-destructive/40",
        local.class,
      )}
      {...rest}
    />
  )
}

const inputGroupAddonVariants = cva(
  "flex h-auto cursor-text items-center justify-center gap-2 py-1.5 text-sm font-medium text-muted-foreground select-none group-data-[disabled=true]/input-group:opacity-50 [&>kbd]:rounded-full [&>svg:not([class*='size-'])]:size-3.5",
  {
    variants: {
      align: {
        "inline-start":
          "order-first pl-3.5 pr-2 has-[>button]:ml-[-0.45rem] has-[>kbd]:ml-[-0.35rem]",
        "inline-end":
          "order-last pl-2 pr-3.5 has-[>button]:mr-[-0.45rem] has-[>kbd]:mr-[-0.35rem]",
        "block-start":
          "order-first w-full justify-start px-3 pt-3 group-has-[>input]/input-group:pt-2.5 [.border-b]:pb-3",
        "block-end":
          "order-last w-full justify-start px-3 pb-3 group-has-[>input]/input-group:pb-2.5 [.border-t]:pt-3",
      },
    },
    defaultVariants: {
      align: "inline-start",
    },
  },
)

function InputGroupAddon(
  props: JSX.HTMLAttributes<HTMLDivElement> &
    VariantProps<typeof inputGroupAddonVariants>,
) {
  const [local, rest] = splitProps(props, ["class", "align"])

  return (
    <div
      role="group"
      data-slot="input-group-addon"
      data-align={local.align || "inline-start"}
      class={cn(inputGroupAddonVariants({ align: local.align }), local.class)}
      onClick={(event) => {
        if ((event.target as HTMLElement).closest("button")) {
          return
        }

        event.currentTarget.parentElement?.querySelector("input")?.focus()
      }}
      {...rest}
    />
  )
}

const inputGroupButtonVariants = cva("flex items-center gap-2 text-sm", {
  variants: {
    size: {
      xs: "h-6 gap-1 border px-2 text-xs font-medium has-[>svg]:px-2 [&>svg:not([class*='size-'])]:size-3.5",
      sm: "h-8 gap-1.5 px-2.5 has-[>svg]:px-2.5",
      "icon-xs": "size-6 min-w-0 border p-0 has-[>svg]:p-0",
      "icon-sm": "size-8 min-w-0 p-0 has-[>svg]:p-0",
    },
  },
  defaultVariants: {
    size: "xs",
  },
})

function InputGroupButton(
  props: Omit<ButtonProps, "size" | "type"> & {
    type?: "button" | "submit" | "reset"
  } & VariantProps<typeof inputGroupButtonVariants>,
) {
  const [local, rest] = splitProps(props, [
    "class",
    "type",
    "variant",
    "size",
  ])

  return (
    <Button
      type={local.type ?? "button"}
      data-size={local.size || "xs"}
      variant={local.variant || "ghost"}
      class={cn(inputGroupButtonVariants({ size: local.size }), local.class)}
      {...rest}
    />
  )
}

function InputGroupText(props: JSX.HTMLAttributes<HTMLSpanElement>) {
  const [local, rest] = splitProps(props, ["class"])

  return (
    <span
      data-slot="input-group-text"
      class={cn(
        "flex items-center gap-2 text-sm text-muted-foreground [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-3.5",
        local.class,
      )}
      {...rest}
    />
  )
}

function InputGroupInput(props: JSX.InputHTMLAttributes<HTMLInputElement>) {
  const [local, rest] = splitProps(props, ["class"])

  return (
    <Input
      data-slot="input-group-control"
      class={cn(
        "h-9 flex-1 rounded-none border-0 bg-transparent px-0 shadow-none focus-visible:ring-0 dark:bg-transparent",
        local.class,
      )}
      {...rest}
    />
  )
}

function InputGroupTextarea(
  props: JSX.TextareaHTMLAttributes<HTMLTextAreaElement>,
) {
  const [local, rest] = splitProps(props, ["class"])

  return (
    <Textarea
      data-slot="input-group-control"
      class={cn(
        "flex-1 resize-none rounded-none border-0 bg-transparent py-3 shadow-none focus-visible:ring-0 dark:bg-transparent",
        local.class,
      )}
      {...rest}
    />
  )
}

export {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupText,
  InputGroupInput,
  InputGroupTextarea,
}
