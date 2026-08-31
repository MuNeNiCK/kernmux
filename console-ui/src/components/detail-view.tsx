import { For, type JSX } from "solid-js"

export type SummaryItem = { label: string; value: JSX.Element; detail?: JSX.Element; tone?: "default" | "success" | "warning" | "danger" }
export type DetailItem = { label: string; value: JSX.Element }
const toneClass = (tone?: SummaryItem["tone"]) => tone === "success" ? "text-success" : tone === "warning" ? "text-warning" : tone === "danger" ? "text-destructive" : "text-foreground"

export default function DetailView(props: { summary: SummaryItem[]; children?: JSX.Element; testId?: string }) {
  return <div class="grid min-w-0 gap-5" data-testid={props.testId}><section class="grid grid-cols-1 border-y border-border min-[760px]:grid-cols-2 min-[1120px]:grid-cols-4" aria-label="Summary">
    <For each={props.summary}>{item => <div class="min-w-0 border-border px-4 py-3.5 min-[760px]:border-r min-[760px]:nth-[2n]:border-r-0 min-[1120px]:nth-[2n]:border-r min-[1120px]:last:border-r-0"><span class="block truncate text-xs text-muted-foreground">{item.label}</span><strong class={`mt-1 block truncate text-sm font-bold ${toneClass(item.tone)}`}>{item.value}</strong><small class="mt-0.5 block truncate text-xs text-muted-foreground">{item.detail ?? "\u00a0"}</small></div>}</For>
  </section>{props.children}</div>
}

export function DetailSection(props: { title: string; meta?: JSX.Element; children: JSX.Element; class?: string }) {
  return <section class={`min-w-0 border-t border-border ${props.class ?? ""}`}><div class="flex min-h-11 items-center justify-between gap-4"><h2 class="m-0 text-[15px] font-[750]">{props.title}</h2><span class="text-xs font-semibold text-muted-foreground">{props.meta}</span></div><div class="border-t border-border">{props.children}</div></section>
}

export function DetailList(props: { items: DetailItem[]; columns?: 1 | 2 }) {
  return <dl class={`m-0 grid grid-cols-1 ${props.columns === 2 ? "min-[760px]:grid-cols-2" : ""}`}><For each={props.items}>{item => <div class="min-w-0 border-b border-border px-3.5 py-3 min-[760px]:border-r min-[760px]:even:border-r-0"><dt class="text-xs text-muted-foreground">{item.label}</dt><dd class="mt-1.5 min-w-0 break-words text-[13px] font-semibold">{item.value}</dd></div>}</For></dl>
}
