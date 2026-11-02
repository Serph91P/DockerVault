import { Select } from '@base-ui/react/select'
import { Combobox } from '@base-ui/react/combobox'
import { Check, ChevronsUpDown } from 'lucide-react'
import { useId } from 'react'

interface ChoiceProps<Value extends string> {
  label: string
  value: Value
  onValueChange: (value: Value) => void
  options: { value: Value; label: string }[]
}
const triggerStyle = 'flex min-h-11 w-full items-center justify-between gap-2 rounded-lg border border-dark-500 bg-dark-900 px-3 py-2 text-sm text-dark-100'
const popupStyle = 'max-h-[min(22rem,var(--available-height))] min-w-[var(--anchor-width)] overflow-y-auto rounded-lg border border-dark-500 bg-dark-800 p-1 text-sm text-dark-100 shadow-xl'
const itemStyle = 'flex min-h-11 items-center gap-2 rounded px-3 py-2 data-highlighted:bg-dark-600'

export function ChoiceSelect<Value extends string>({ label, value, onValueChange, options }: ChoiceProps<Value>) {
  return <Select.Root items={options} value={value} onValueChange={(next) => { if (next !== null) onValueChange(next) }}>
    <Select.Label className="text-sm text-dark-200">{label}</Select.Label>
    <Select.Trigger className={triggerStyle}><Select.Value /><Select.Icon><ChevronsUpDown className="size-4" /></Select.Icon></Select.Trigger>
    <Select.Portal><Select.Positioner className="z-[110]" sideOffset={4} alignItemWithTrigger={false}>
      <Select.Popup className={popupStyle} data-overlay="select"><Select.List>{options.map((option) => <Select.Item key={option.value} value={option.value} className={itemStyle}><Select.ItemIndicator><Check className="size-4" /></Select.ItemIndicator><Select.ItemText>{option.label}</Select.ItemText></Select.Item>)}</Select.List></Select.Popup>
    </Select.Positioner></Select.Portal>
  </Select.Root>
}
export function SearchSelect<Value extends string>({ label, value, onValueChange, options }: ChoiceProps<Value>) {
  const id = useId()
  const items = Combobox.createItems(options, { getValue: (item) => item.value, getLabel: (item) => item.label })
  return <Combobox.Root items={items} value={value} onValueChange={(next, details) => {
    if (next !== null) onValueChange(next)
    // These choices are non-clearable; a closed input must not consume its parent's Escape.
    else if (details.reason === 'escape-key') details.allowPropagation()
  }}>
    <label htmlFor={id} className="block text-sm text-dark-200">{label}</label>
    <Combobox.InputGroup className="flex items-center gap-1"><Combobox.Input id={id} aria-label={label} className={triggerStyle} /><Combobox.Trigger aria-label={`Show ${label.toLowerCase()} options`} className="min-h-11 px-3"><ChevronsUpDown className="size-4" /></Combobox.Trigger></Combobox.InputGroup>
    <Combobox.Portal><Combobox.Positioner className="z-[110]" sideOffset={4}>
      <Combobox.Popup className={popupStyle} data-overlay="combobox"><Combobox.Empty className="p-3">No matching options</Combobox.Empty><Combobox.List>{(item: ChoiceProps<Value>['options'][number]) => <Combobox.Item key={item.value} value={item.value} className={itemStyle}><Combobox.ItemIndicator><Check className="size-4" /></Combobox.ItemIndicator>{item.label}</Combobox.Item>}</Combobox.List></Combobox.Popup>
    </Combobox.Positioner></Combobox.Portal>
  </Combobox.Root>
}
