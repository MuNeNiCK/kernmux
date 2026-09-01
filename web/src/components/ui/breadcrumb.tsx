import type { VoidProps } from "solid-js"
import { splitProps, type ComponentProps, type ValidComponent } from "solid-js"
import { Breadcrumbs as BreadcrumbsPrimitive } from "@kobalte/core/breadcrumbs"
import ChevronRightIcon from "lucide-solid/icons/chevron-right"
import MoreHorizontalIcon from "lucide-solid/icons/ellipsis"

import { cx } from "@/lib/cva"

export type BreadcrumbsProps<T extends ValidComponent = "nav"> = ComponentProps<
  typeof BreadcrumbsPrimitive<T>
>

export const Breadcrumbs = <T extends ValidComponent = "nav">(
  props: BreadcrumbsProps<T>,
) => {
  return (
    <BreadcrumbsPrimitive
      data-slot="breadcrumbs"
      separator={
        <svg
          xmlns="http://www.w3.org/2000/svg"
          class="size-4"
          viewBox="0 0 24 24"
        >
          <path
            fill="none"
            stroke="currentColor"
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="m9 18l6-6l-6-6"
          />
        </svg>
      }
      {...props}
    />
  )
}

export type BreadcrumbListProps = ComponentProps<"ol">

export const BreadcrumbList = (props: BreadcrumbListProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <ol
      data-slot="breadcrumb-list"
      class={cx(
        "flex flex-wrap items-center gap-1.5 text-sm break-words text-muted-foreground sm:gap-2",
        props.class,
      )}
      {...rest}
    />
  )
}

export type BreadcrumbsItemProps = ComponentProps<"li">

export const BreadcrumbsItem = (props: BreadcrumbsItemProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <li
      data-slot="breadcrumb-item"
      class={cx("inline-flex items-center gap-1.5", props.class)}
      {...rest}
    />
  )
}

export type BreadcrumbsLinkProps<T extends ValidComponent = "a"> =
  ComponentProps<typeof BreadcrumbsPrimitive.Link<T>>

export const BreadcrumbsLink = <T extends ValidComponent = "a">(
  props: BreadcrumbsLinkProps<T>,
) => {
  const [, rest] = splitProps(props as BreadcrumbsLinkProps, ["class"])

  return (
    <BreadcrumbsPrimitive.Link
      data-slot="breadcrumb-link"
      class={cx(
        "rounded-md px-1 transition-colors hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/35 focus-visible:outline-none data-[current]:text-foreground data-[current]:font-normal",
        props.class,
      )}
      {...rest}
    />
  )
}

export type BreadcrumbsSeparatorProps<T extends ValidComponent = "span"> =
  ComponentProps<typeof BreadcrumbsPrimitive.Separator<T>>

export const BreadcrumbsSeparator = <T extends ValidComponent = "span">(
  props: BreadcrumbsSeparatorProps<T>,
) => {
  const [, rest] = splitProps(props as BreadcrumbsSeparatorProps, [
    "class",
    "children",
  ])

  return (
    <BreadcrumbsPrimitive.Separator
      data-slot="breadcrumb-separator"
      role="presentation"
      aria-hidden="true"
      class={cx("text-muted-foreground/70 [&>svg]:size-3.5", props.class)}
      {...rest}
    >
      {props.children ?? <ChevronRightIcon />}
    </BreadcrumbsPrimitive.Separator>
  )
}

export type BreadcrumbsEllipsisProps = VoidProps<ComponentProps<"span">>

export const BreadcrumbsEllipsis = (props: BreadcrumbsEllipsisProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <span
      data-slot="breadcrumb-ellipsis"
      role="presentation"
      aria-hidden="true"
      class={cx(
        "flex size-8 items-center justify-center rounded-md hover:bg-accent",
        props.class,
      )}
      {...rest}
    >
      <MoreHorizontalIcon class="size-4" />
      <span class="sr-only">More</span>
    </span>
  )
}


export const Breadcrumb = Breadcrumbs
export const BreadcrumbItem = BreadcrumbsItem
export const BreadcrumbLink = BreadcrumbsLink
export const BreadcrumbSeparator = BreadcrumbsSeparator
export const BreadcrumbEllipsis = BreadcrumbsEllipsis
export type BreadcrumbPageProps = ComponentProps<"span">
export const BreadcrumbPage = (props: BreadcrumbPageProps) => {
  const [, rest] = splitProps(props, ["class"])
  return <span data-slot="breadcrumb-page" role="link" aria-disabled="true" aria-current="page" class={cx("font-medium text-foreground", props.class)} {...rest} />
}
