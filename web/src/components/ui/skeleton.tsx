import type { ComponentProps } from "solid-js"
import { splitProps } from "solid-js"

import { cx } from "@/lib/cva"

export type SkeletonProps = ComponentProps<"div">

export const Skeleton = (props: SkeletonProps) => {
  const [, rest] = splitProps(props, ["class"])

  return (
    <div
      data-slot="skeleton"
      class={cx("animate-pulse rounded-md bg-border/70", props.class)}
      {...rest}
    />
  )
}
