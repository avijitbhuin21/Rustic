// Searchable, scrollable single-select for long option lists (models,
// providers, projects). Never taller than the viewport and scrolls with the
// mouse wheel even inside Dialogs (issue #10).
import * as React from 'react';
import { Check, ChevronDown } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList,
} from '@/components/ui/command';
import { cn } from '@/lib/utils';

/** Normalise `options` (strings or {value,label}) into {value,label} objects. */
function toOptions(list) {
  return (list || []).map((o) =>
    typeof o === 'string' ? { value: o, label: o } : { value: o.value, label: o.label ?? o.value },
  );
}

/**
 * Searchable select.
 * - `options`: array of strings or `{ value, label }`; or pass `groups`:
 *   `[{ label, options }]`.
 * - A `value` missing from the options is still shown (e.g. saved custom id).
 */
export function SearchableSelect({
  value,
  onValueChange,
  options,
  groups,
  placeholder = 'Select…',
  searchPlaceholder = 'Search…',
  emptyText = 'No matches.',
  disabled = false,
  className,
  contentClassName,
  align = 'start',
}) {
  const [open, setOpen] = React.useState(false);

  const normGroups = React.useMemo(() => {
    const gs = groups
      ? groups.map((g) => ({ label: g.label, options: toOptions(g.options) }))
      : [{ label: null, options: toOptions(options) }];
    const all = gs.flatMap((g) => g.options);
    if (value && !all.some((o) => o.value === value)) {
      gs.unshift({ label: null, options: [{ value, label: value }] });
    }
    return gs;
  }, [groups, options, value]);

  const selectedLabel = React.useMemo(() => {
    for (const g of normGroups) {
      const hit = g.options.find((o) => o.value === value);
      if (hit) return hit.label;
    }
    return null;
  }, [normGroups, value]);

  return (
    // `modal` gives the popover its own scroll lock, so the wheel works when
    // it is opened from inside a Dialog (the dialog's lock otherwise eats it).
    <Popover open={open} onOpenChange={setOpen} modal>
      <PopoverTrigger asChild disabled={disabled}>
        <button
          type="button"
          role="combobox"
          aria-expanded={open}
          disabled={disabled}
          className={cn(
            'flex h-8 w-full min-w-0 items-center justify-between gap-2 rounded-lg border border-input bg-transparent px-2.5 text-left text-xs outline-none transition-colors focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30',
            className,
          )}
        >
          <span className={cn('truncate', !selectedLabel && 'text-muted-foreground')}>
            {selectedLabel ?? placeholder}
          </span>
          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align={align}
        collisionPadding={8}
        className={cn('w-(--radix-popover-trigger-width) min-w-56 gap-0 p-0', contentClassName)}
      >
        <Command>
          <CommandInput placeholder={searchPlaceholder} className="text-xs" />
          <CommandList className="max-h-[min(320px,calc(var(--radix-popover-content-available-height)-3rem))] overscroll-contain [scrollbar-width:thin] [&::-webkit-scrollbar]:block">
            <CommandEmpty className="py-4 text-center text-xs text-muted-foreground">{emptyText}</CommandEmpty>
            {normGroups.map((g, gi) => (
              <CommandGroup key={`${g.label ?? ''}-${gi}`} heading={g.label || undefined}>
                {g.options.map((o) => (
                  <CommandItem
                    key={o.value}
                    value={`${o.label} ${o.value}`}
                    onSelect={() => {
                      onValueChange?.(o.value);
                      setOpen(false);
                    }}
                    className="text-xs"
                  >
                    <Check className={cn('size-3.5 shrink-0', o.value === value ? 'opacity-100' : 'opacity-0')} />
                    <span className="truncate">{o.label}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

export default SearchableSelect;
