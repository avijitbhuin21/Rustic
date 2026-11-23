import * as React from "react"
import { cva } from "class-variance-authority";
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"
import { InkLayer, useInkKit } from "@/components/ui/ink"

const INKED_VARIANTS = new Set(["default", "secondary", "destructive", "outline"])

const badgeVariants = cva(
  "group/badge inline-flex h-5 w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-4xl border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-all focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3!",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a]:hover:bg-primary/80",
        secondary:
          "bg-secondary text-secondary-foreground [a]:hover:bg-secondary/80",
        destructive:
          "bg-destructive/10 text-destructive focus-visible:ring-destructive/20 dark:bg-destructive/20 dark:focus-visible:ring-destructive/40 [a]:hover:bg-destructive/20",
        outline:
          "border-border text-foreground [a]:hover:bg-muted [a]:hover:text-muted-foreground",
        ghost:
          "hover:bg-muted hover:text-muted-foreground dark:hover:bg-muted/50",
        link: "text-primary underline-offset-4 hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  children,
  ...props
}) {
  const Comp = asChild ? Slot.Root : "span"
  const inked = useInkKit() !== null && INKED_VARIANTS.has(variant)
  // The badge sizes its direct <svg> children with !important, so the drawing
  // sits in a wrapper instead of directly inside it.
  const ink = inked ? (
    <span data-ink-wrap="" aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10">
      <InkLayer recipe="badge" params={{ variant, size: "default" }} />
    </span>
  ) : null

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}>
      {inked && asChild ? <Slot.Slottable>{children}</Slot.Slottable> : children}
      {ink}
    </Comp>
  );
}

export { Badge, badgeVariants }
