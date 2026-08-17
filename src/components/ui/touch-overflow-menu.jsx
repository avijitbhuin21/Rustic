import React from 'react';
import { MoreHorizontal } from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

/**
 * Collapses a row's action buttons into a single ⋯ menu, for touch devices where
 * three or more icon buttons in a row are too cramped to hit reliably.
 *
 * `items` is `{ label, icon, onSelect, destructive }[]`. Render this only when
 * the pointer is coarse; the caller keeps its hover cluster for mouse users.
 */
export function TouchOverflowMenu({ items, label = 'Actions', className }) {
  const usable = (items || []).filter(Boolean);
  if (usable.length === 0) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={label}
          onClick={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
          className={cn(
            'flex size-8 shrink-0 items-center justify-center rounded text-muted-foreground active:bg-foreground/10',
            className,
          )}
        >
          <MoreHorizontal className="size-4" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[180px]">
        {usable.map(({ label: itemLabel, icon: Icon, onSelect, destructive }) => (
          <DropdownMenuItem
            key={itemLabel}
            variant={destructive ? 'destructive' : undefined}
            onSelect={(e) => {
              e.stopPropagation?.();
              onSelect?.();
            }}
          >
            {Icon ? <Icon className="size-4" /> : null}
            {itemLabel}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export default TouchOverflowMenu;
