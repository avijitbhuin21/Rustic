import React from 'react';
import logoDark from '@/assets/logo-dark.png';
import logoLight from '@/assets/logo-light.png';
import { cn } from '@/lib/utils';

// Picks the logo variant via the `.dark` class on <html>, which ThemeBridge
// toggles whenever the active theme changes. Two stacked imgs (one hidden in
// each mode) avoid a React re-render on every theme switch.
export function Logo({ className, alt = 'Rustic' }) {
  return (
    <>
      <img
        src={logoDark}
        alt={alt}
        aria-hidden
        draggable={false}
        className={cn('hidden select-none dark:block', className)}
      />
      <img
        src={logoLight}
        alt={alt}
        aria-hidden
        draggable={false}
        className={cn('block select-none dark:hidden', className)}
      />
    </>
  );
}
