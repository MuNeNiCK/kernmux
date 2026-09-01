import type { ComponentProps, ValidComponent } from "solid-js"
import { splitProps } from "solid-js"
import { Tabs as TabsPrimitive } from "@kobalte/core/tabs"
import type { VariantProps } from "cva"

import { cva, cx } from "@/lib/cva"

export type TabsProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof TabsPrimitive<T>
>

export const Tabs = <T extends ValidComponent = "div">(props: TabsProps<T>) => {
  const [, rest] = splitProps(props as TabsProps, ["class"])

  return (
    <TabsPrimitive
      data-slot="tabs"
      class={cx(
        "group/tabs flex gap-2 data-[orientation=horizontal]:flex-col data-horizontal:flex-col data-[orientation=vertical]:flex-row",
        props.class,
      )}
      {...rest}
    />
  )
}

export const tabsListVariants = cva({
  base: "group/tabs-list inline-flex w-fit items-center text-muted-foreground group-data-[orientation=vertical]/tabs:h-fit group-data-[orientation=vertical]/tabs:flex-col group-data-[orientation=vertical]/tabs:items-stretch group-data-vertical/tabs:h-fit group-data-vertical/tabs:flex-col group-data-vertical/tabs:items-stretch",
  variants: {
    variant: {
      default:
        "gap-5 group-data-[orientation=horizontal]/tabs:h-10 group-data-[orientation=horizontal]/tabs:justify-start group-data-[orientation=horizontal]/tabs:border-b group-data-[orientation=vertical]/tabs:border-r group-data-horizontal/tabs:h-10 group-data-horizontal/tabs:justify-start group-data-horizontal/tabs:border-b group-data-vertical/tabs:border-r",
      line:
        "gap-5 group-data-[orientation=horizontal]/tabs:h-10 group-data-[orientation=horizontal]/tabs:justify-start group-data-[orientation=horizontal]/tabs:border-b group-data-[orientation=vertical]/tabs:border-r group-data-horizontal/tabs:h-10 group-data-horizontal/tabs:justify-start group-data-horizontal/tabs:border-b group-data-vertical/tabs:border-r",
      segmented:
        "w-fit gap-0 rounded-full border border-border bg-card p-0.5 group-data-[orientation=horizontal]/tabs:h-10 group-data-horizontal/tabs:h-10",
    },
  },
  defaultVariants: {
    variant: "default",
  },
})

export type TabsListProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof TabsPrimitive.List<T>
> &
  VariantProps<typeof tabsListVariants>

export const TabsList = <T extends ValidComponent = "div">(
  props: TabsListProps<T>,
) => {
  const [, rest] = splitProps(props as TabsListProps, ["class", "variant"])

  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      data-variant={props.variant ?? "default"}
      class={tabsListVariants({
        variant: props.variant,
        class: props.class,
      })}
      {...rest}
    />
  )
}

export type TabsTriggerProps<T extends ValidComponent = "button"> =
  ComponentProps<typeof TabsPrimitive.Trigger<T>>

export const TabsTrigger = <T extends ValidComponent = "button">(
  props: TabsTriggerProps<T>,
) => {
  const [, rest] = splitProps(props as TabsTriggerProps, ["class"])

  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      class={cx(
        "relative inline-flex items-center justify-center gap-1.5 whitespace-nowrap border-transparent px-1 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/35 focus-visible:outline-none disabled:pointer-events-none disabled:opacity-50 data-[selected]:text-foreground data-active:text-foreground dark:hover:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        "group-data-[orientation=horizontal]/tabs:-mb-px group-data-[orientation=horizontal]/tabs:h-10 group-data-[orientation=horizontal]/tabs:border-b-2 group-data-[orientation=horizontal]/tabs:data-[selected]:border-primary group-data-horizontal/tabs:-mb-px group-data-horizontal/tabs:h-10 group-data-horizontal/tabs:border-b-2 group-data-horizontal/tabs:data-active:border-primary",
        "group-data-[orientation=vertical]/tabs:w-full group-data-[orientation=vertical]/tabs:justify-start group-data-[orientation=vertical]/tabs:border-r-2 group-data-[orientation=vertical]/tabs:border-b-0 group-data-[orientation=vertical]/tabs:px-3 group-data-[orientation=vertical]/tabs:py-2 group-data-[orientation=vertical]/tabs:data-[selected]:border-primary group-data-vertical/tabs:w-full group-data-vertical/tabs:justify-start group-data-vertical/tabs:border-r-2 group-data-vertical/tabs:border-b-0 group-data-vertical/tabs:px-3 group-data-vertical/tabs:py-2 group-data-vertical/tabs:data-active:border-primary",
        "group-data-[variant=segmented]/tabs-list:-mb-0 group-data-[variant=segmented]/tabs-list:h-8 group-data-[variant=segmented]/tabs-list:rounded-full group-data-[variant=segmented]/tabs-list:border group-data-[variant=segmented]/tabs-list:border-transparent group-data-[variant=segmented]/tabs-list:px-3 group-data-[variant=segmented]/tabs-list:data-[selected]:border-primary group-data-[variant=segmented]/tabs-list:data-[selected]:bg-primary/10 group-data-[variant=segmented]/tabs-list:data-[selected]:text-primary group-data-[variant=segmented]/tabs-list:data-active:border-primary group-data-[variant=segmented]/tabs-list:data-active:bg-primary/10 group-data-[variant=segmented]/tabs-list:data-active:text-primary",
        props.class,
      )}
      {...rest}
    />
  )
}

export type TabsContentProps<T extends ValidComponent = "div"> = ComponentProps<
  typeof TabsPrimitive.Content<T>
>

export const TabsContent = <T extends ValidComponent = "div">(
  props: TabsContentProps<T>,
) => {
  const [, rest] = splitProps(props as TabsContentProps, ["class"])

  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      class={cx("flex-1 pt-4 outline-none", props.class)}
      {...rest}
    />
  )
}

export type TabsIndicatorProps<T extends ValidComponent = "div"> =
  ComponentProps<typeof TabsPrimitive.Indicator<T>>

export const TabsIndicator = <T extends ValidComponent = "div">(
  props: TabsIndicatorProps<T>,
) => {
  const [, rest] = splitProps(props as TabsIndicatorProps, ["class"])

  return (
    <TabsPrimitive.Indicator
      data-slot="tabs-indicator"
      class={cx(
        "bg-background dark:bg-input/30 dark:border-input peer-focus-visible:border-ring peer-focus-visible:ring-ring/50 peer-focus-visible:outline-ring absolute inset-0 rounded-lg border border-transparent shadow-sm transition-[box-shadow,transform,width,height] duration-200 peer-focus-visible:ring-[3px] peer-focus-visible:outline-1",
        props.class,
      )}
      {...rest}
    />
  )
}
