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

/** Shared look of every connected row block (SettingsSection, RowGroup, agent Section). */
export const GROUP_BOX = 'rounded-xl border border-border/50 bg-muted/20 overflow-hidden';
/** Small uppercase heading above a connected block. */
export const GROUP_TITLE = 'text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70';

/**
 * Connected block of arbitrary rows (lists of servers, tools, shortcuts…),
 * same container + dividers as SettingsSection. Children are usually GroupRows.
 * `inset` flushes the rows into a surrounding bordered card (e.g. an agent
 * `Section`) instead of drawing a second box inside it.
 */
export function RowGroup({ title, actions, inset = false, className, children }) {
  const rows = inset ? (
    <div
      className={[
        '-mx-4 divide-y divide-border/40 border-y border-border/40',
        'first:-mt-3 first:border-t-0 last:-mb-3 last:border-b-0',
        className,
      ].filter(Boolean).join(' ')}
    >
      {children}
    </div>
  ) : (
    <div className={[GROUP_BOX, 'divide-y divide-border/40', className].filter(Boolean).join(' ')}>
      {children}
    </div>
  );
  if (!title && !actions) return rows;
  return (
    <div className="mb-4">
      <div className="mb-2 flex items-center gap-2 px-1">
        {title && <h3 className={GROUP_TITLE}>{title}</h3>}
        {actions && <div className="ml-auto flex items-center gap-1.5">{actions}</div>}
      </div>
      {rows}
    </div>
  );
}

/** One row of a RowGroup: content left, controls right, filterable via data-setting-row. */
export function GroupRow({ className, children, ...rest }) {
  return (
    <div
      data-setting-row
      className={['flex items-center gap-3 px-3 py-2.5', className].filter(Boolean).join(' ')}
      {...rest}
    >
      {children}
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
      <h3 className={`mb-2 px-1 ${GROUP_TITLE}`}>
        {title}
      </h3>
      <div className={`${GROUP_BOX} divide-y divide-border/40 px-3`}>
        <SettingsFilterContext.Provider value={innerQuery}>
          {children}
        </SettingsFilterContext.Provider>
      </div>
    </section>
  );
}
