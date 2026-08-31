import { splitProps, type JSX } from "solid-js"
import { cn } from "@/lib/utils"

function ScrollArea(props: JSX.HTMLAttributes<HTMLDivElement>) {
  const [l, r] = splitProps(props, ["class", "children"])

  return (
    <div data-slot="scroll-area" class={cn("relative overflow-hidden", l.class)} {...r}>
      <div
        data-slot="scroll-area-viewport"
        class="size-full overflow-auto rounded-[inherit] transition-[color,box-shadow] outline-none [scrollbar-width:none] focus-visible:ring-[3px] focus-visible:ring-ring/35 focus-visible:outline-none [&::-webkit-scrollbar]:hidden"
      >
        {l.children}
      </div>
      <ScrollBar />
      <div data-slot="scroll-area-corner" />
    </div>
  )
}

function ScrollBar(
  props: JSX.HTMLAttributes<HTMLDivElement> & {
    orientation?: "horizontal" | "vertical"
  },
) {
  const [l, r] = splitProps(props, ["class", "orientation", "children"])
  const orientation = () => l.orientation ?? "vertical"

  return (
    <div
      data-slot="scroll-area-scrollbar"
      data-orientation={orientation()}
      class={cn(
        "absolute flex touch-none p-px transition-colors select-none",
        orientation() === "horizontal"
          ? "inset-x-0 bottom-0 h-2.5 flex-col border-t border-t-transparent"
          : "top-0 right-0 h-full w-2.5 border-l border-l-transparent",
        l.class,
      )}
      {...r}
    >
      {l.children ?? (
        <div data-slot="scroll-area-thumb" class="relative flex-1 rounded-full bg-muted-foreground/40" />
      )}
    </div>
  )
}

export { ScrollArea, ScrollBar }
