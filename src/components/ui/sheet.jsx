import * as React from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { XIcon } from "lucide-react";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";

// Slide-from-right Sheet built on Radix Dialog. Mirrors the shadcn dialog
// styling but pins the content to a side edge and animates on the X axis.
// Kept minimal — only the right-side variant is wired up because that's all
// the agent kebab menu needs.

function Sheet({ ...props }) {
  return <DialogPrimitive.Root data-slot="sheet" {...props} />;
}

function SheetTrigger({ ...props }) {
  return <DialogPrimitive.Trigger data-slot="sheet-trigger" {...props} />;
}

function SheetClose({ ...props }) {
  return <DialogPrimitive.Close data-slot="sheet-close" {...props} />;
}

function SheetPortal({ ...props }) {
  return <DialogPrimitive.Portal data-slot="sheet-portal" {...props} />;
}

function SheetOverlay({ className, ...props }) {
  return (
    <DialogPrimitive.Overlay
      data-slot="sheet-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-black/30 duration-150 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className,
      )}
      {...props}
    />
  );
}

function SheetContent({
  className,
  children,
  side = "right",
  showCloseButton = true,
  ...props
}) {
  const sideStyles =
    side === "right"
      ? "right-0 top-0 h-full w-[420px] max-w-[90vw] border-l data-open:slide-in-from-right data-closed:slide-out-to-right"
      : "left-0 top-0 h-full w-[420px] max-w-[90vw] border-r data-open:slide-in-from-left data-closed:slide-out-to-left";

  return (
    <SheetPortal>
      <SheetOverlay />
      <DialogPrimitive.Content
        data-slot="sheet-content"
        aria-describedby={undefined}
        className={cn(
          "fixed z-50 flex flex-col gap-0 bg-popover text-popover-foreground shadow-2xl outline-none",
          "duration-200 data-open:animate-in data-closed:animate-out",
          sideStyles,
          className,
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="absolute right-2 top-2"
            >
              <XIcon />
              <span className="sr-only">Close</span>
            </Button>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </SheetPortal>
  );
}

function SheetHeader({ className, ...props }) {
  return (
    <div
      data-slot="sheet-header"
      className={cn(
        "flex shrink-0 flex-col gap-1 border-b border-border/60 px-4 py-3",
        className,
      )}
      {...props}
    />
  );
}

function SheetTitle({ className, ...props }) {
  return (
    <DialogPrimitive.Title
      data-slot="sheet-title"
      className={cn("text-sm font-semibold leading-none", className)}
      {...props}
    />
  );
}

function SheetDescription({ className, ...props }) {
  return (
    <DialogPrimitive.Description
      data-slot="sheet-description"
      className={cn("text-xs text-muted-foreground", className)}
      {...props}
    />
  );
}

function SheetBody({ className, ...props }) {
  return (
    <div
      data-slot="sheet-body"
      className={cn("min-h-0 flex-1 overflow-auto", className)}
      {...props}
    />
  );
}

export {
  Sheet,
  SheetTrigger,
  SheetClose,
  SheetPortal,
  SheetOverlay,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
  SheetBody,
};
