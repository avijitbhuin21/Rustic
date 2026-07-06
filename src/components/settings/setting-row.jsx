import React, { createContext, useContext } from 'react';
import { Label } from '@/components/ui/label';

// Shared filter context — settings-panel.jsx provides the current query; rows
// and sections read it and hide themselves when their text doesn't match.
const SettingsFilterContext = createContext('');

export function SettingsFilterProvider({ value, children }) {
  return (
    <SettingsFilterContext.Provider value={value || ''}>
      {children}
    </SettingsFilterContext.Provider>
  );
}

function matchesQuery(query, ...parts) {
  if (!query) return true;
  const q = query.toLowerCase();
  return parts.some((p) => typeof p === 'string' && p.toLowerCase().includes(q));
}

export function SettingRow({ label, description, children, htmlFor }) {
  const query = useContext(SettingsFilterContext);
  if (!matchesQuery(query, label, description)) return null;
  return (
    <div data-setting-row className="flex items-start justify-between gap-4 py-3">
      <div className="flex min-w-0 flex-col">
        <Label htmlFor={htmlFor} className="text-[13px] font-normal">
          {label}
        </Label>
        {description && (
          <span className="mt-0.5 text-[12px] italic leading-snug text-muted-foreground">{description}</span>
        )}
      </div>
      <div className="flex shrink-0 items-center">{children}</div>
    </div>
  );
}

// When a section's title itself matches the query, every row inside should
// show (so searching "Cursor" reveals the whole Cursor section). We do that
// by overriding the inner filter context to empty for matched sections. When
// the title doesn't match, rows filter individually and the section hides
// itself via :has() if none survive.
export function SettingsSection({ title, children }) {
  const query = useContext(SettingsFilterContext);
  const titleMatches = matchesQuery(query, title);
  const innerQuery = titleMatches ? '' : query;
  const anchor = String(title).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');

  return (
    <section data-settings-anchor={anchor} className="mb-6 [&:not(:has([data-setting-row]))]:hidden">
      <h3 className="mb-2 px-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70">
        {title}
      </h3>
      <div className="rounded-xl border border-border/50 bg-muted/20 divide-y divide-border/40 overflow-hidden px-3">
        <SettingsFilterContext.Provider value={innerQuery}>
          {children}
        </SettingsFilterContext.Provider>
      </div>
    </section>
  );
}
