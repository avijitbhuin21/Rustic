import React from 'react';
import { Label } from '@/components/ui/label';

export function SettingRow({ label, description, children, htmlFor }) {
  return (
    <div className="flex items-start justify-between gap-4 py-2">
      <div className="flex min-w-0 flex-col">
        <Label htmlFor={htmlFor} className="text-xs">
          {label}
        </Label>
        {description && (
          <span className="mt-0.5 text-[11px] text-muted-foreground">{description}</span>
        )}
      </div>
      <div className="flex shrink-0 items-center">{children}</div>
    </div>
  );
}

export function SettingsSection({ title, children }) {
  return (
    <section className="mb-4">
      <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">{title}</h3>
      <div className="divide-y divide-border/40">{children}</div>
    </section>
  );
}
