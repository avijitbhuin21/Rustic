import React from 'react';
import antigravityLogo from '@/assets/agent-logos/antigravity.svg';
import claudeCodeLogo from '@/assets/agent-logos/claude-code.svg';
import codexLogo from '@/assets/agent-logos/codex.svg';
import { cn } from '@/lib/utils';

// Official brand marks for the external CLI agents, taken from the MIT-licensed
// @lobehub/icons-static-svg set (colour variants). Rendered as <img> — same
// pattern as `components/logo.jsx` — because Antigravity's mark is a masked
// gradient composition that can't be expressed as one tintable path. Codex
// ships on a white plate upstream; that plate is dropped so the gradient knot
// reads on both light and dark themes.

const LOGOS = {
  claude: { src: claudeCodeLogo, alt: 'Claude Code' },
  codex: { src: codexLogo, alt: 'Codex' },
  agy: { src: antigravityLogo, alt: 'Antigravity' },
};

// Brand logo for one agent id (`claude` / `codex` / `agy`); null when unknown.
export function AgentLogo({ agent, className }) {
  const logo = LOGOS[agent];
  if (!logo) return null;
  return (
    <img
      src={logo.src}
      alt={logo.alt}
      aria-hidden
      draggable={false}
      className={cn('select-none', className)}
    />
  );
}
