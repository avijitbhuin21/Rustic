import React from 'react';
import { TerminalPanel } from '@/components/terminal/terminal-panel';

export function BottomPanelHost() {
  // Bottom panel currently only hosts the terminal — Problems / Output stubs
  // were removed to give the terminal the full vertical space. If we add those
  // back as real surfaces later, re-introduce the tab strip here.
  return (
    <div className="flex h-full w-full flex-col bg-background">
      <TerminalPanel location="bottom" />
    </div>
  );
}
