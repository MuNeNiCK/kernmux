import { For, Show, createSignal, type JSX } from "solid-js"
import ChevronLeftIcon from "lucide-solid/icons/chevron-left"

import { Button } from "@/components/ui/button"

export type ReviewItem = { label: string; value: JSX.Element }

export default function FormFlow(props: { title: string; description: string; children: JSX.Element; review: () => ReviewItem[]; canReview: () => boolean; busy?: boolean; applyLabel: string; onApply: () => unknown | Promise<unknown>; testId?: string }) {
  const [reviewing, setReviewing] = createSignal(false)
  return <section class="grid gap-4 border-t border-border" data-testid={props.testId}>
    <div class="flex min-h-12 items-center justify-between gap-4"><div><h2 class="m-0 text-[15px] font-[750]">{props.title}</h2><p class="mt-1 text-xs text-muted-foreground">{props.description}</p></div><span class="text-xs font-semibold text-muted-foreground">{reviewing() ? "Review" : "Configure"}</span></div>
    <div class="border-t border-border pt-4"><Show when={!reviewing()} fallback={<dl class="grid grid-cols-1 border-y min-[760px]:grid-cols-2"><For each={props.review()}>{item => <div class="border-b border-border px-3.5 py-3 min-[760px]:border-r min-[760px]:even:border-r-0"><dt class="text-xs text-muted-foreground">{item.label}</dt><dd class="mt-1.5 break-words text-sm font-semibold">{item.value}</dd></div>}</For></dl>}>{props.children}</Show></div>
    <div class="flex justify-end gap-2 border-t border-border pt-3"><Show when={reviewing()}><Button type="button" variant="outline" size="sm" onClick={() => setReviewing(false)}><ChevronLeftIcon />Back</Button></Show><Button type="button" size="sm" disabled={props.busy || (!reviewing() && !props.canReview())} onClick={() => reviewing() ? void props.onApply() : setReviewing(true)}>{reviewing() ? props.applyLabel : "Review changes"}</Button></div>
  </section>
}
