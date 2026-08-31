import { type JSX } from "solid-js"
import InboxIcon from "lucide-solid/icons/inbox"

export default function EmptyState(props: { title: string; description: string; action?: JSX.Element; compact?: boolean }) {
  return <div class={`grid place-items-center px-6 text-center ${props.compact ? "min-h-40 py-8" : "min-h-[360px] py-12"}`} data-testid="empty-state"><div class="grid max-w-md place-items-center"><div class="grid size-12 place-items-center rounded-md border border-primary/20 bg-primary/10 text-primary"><InboxIcon class="size-6" /></div><h2 class="mt-4 text-lg font-[750]">{props.title}</h2><p class="mt-2 text-sm leading-6 text-muted-foreground">{props.description}</p><div class="mt-4">{props.action}</div></div></div>
}
