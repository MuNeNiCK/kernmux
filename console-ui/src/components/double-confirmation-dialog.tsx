import { createMemo, createSignal } from "solid-js"
import ShieldAlertIcon from "lucide-solid/icons/shield-alert"

import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogMedia, AlertDialogPortal, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"

export default function DoubleConfirmation(props: { open: boolean; onOpenChange: (open: boolean) => void; title: string; description: string; confirmation: string; actionLabel: string; busy?: boolean; onConfirm: () => void | Promise<void> }) {
  const [value, setValue] = createSignal("")
  const canConfirm = createMemo(() => value().trim() === props.confirmation)
  const changeOpen = (open: boolean) => { if (!open) setValue(""); props.onOpenChange(open) }
  return <AlertDialog open={props.open} onOpenChange={changeOpen}><AlertDialogPortal><AlertDialogContent><AlertDialogHeader><AlertDialogMedia><ShieldAlertIcon /></AlertDialogMedia><AlertDialogTitle>{props.title}</AlertDialogTitle><AlertDialogDescription>{props.description} Type <strong>{props.confirmation}</strong> to continue.</AlertDialogDescription></AlertDialogHeader><label class="grid gap-2 text-sm font-semibold" for="destructive-confirmation">Confirmation<Input id="destructive-confirmation" value={value()} autocomplete="off" spellcheck={false} onInput={event => setValue(event.currentTarget.value)} /></label><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><Button type="button" variant="destructive" data-testid="confirm-delete" disabled={!canConfirm() || props.busy} onClick={() => void props.onConfirm()}>{props.actionLabel}</Button></AlertDialogFooter></AlertDialogContent></AlertDialogPortal></AlertDialog>
}
