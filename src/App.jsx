import React, { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TooltipProvider } from '@/components/ui/tooltip';
import { Toaster } from '@/components/ui/sonner';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';
import { ConfirmDialogHost } from '@/components/confirm-dialog';
import { CommandPalette } from '@/components/command-palette';
import { TerminalProjectPicker } from '@/components/terminal-project-picker';
import { ThemeBridge } from '@/components/theme-bridge';
import { OnboardingWizard } from '@/components/onboarding/onboarding-wizard';
import { ShortcutCheatsheet } from '@/components/shortcut-cheatsheet';
import { SettingsModal } from '@/components/settings/settings-modal';
import { KeybindingBridge } from '@/components/keybinding-bridge';
import { FontBridge } from '@/components/shell/font-bridge';
import { WindowControls } from '@/components/shell/window-controls';
import { ActivityBar } from '@/components/shell/activity-bar';
import { SidebarHost } from '@/components/shell/sidebar-host';
import { EditorAreaHost } from '@/components/shell/editor-area-host';
import { BottomPanelHost } from '@/components/shell/bottom-panel-host';
import { StatusBar } from '@/components/shell/status-bar';
import AgentPanel from '@/components/agent/agent-panel';
import { useLayout } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import { useGit } from '@/state/git';
import { useAgent } from '@/state/agent';
import { useEditor } from '@/state/editor';
import { useSettings } from '@/state/settings';
import { useTerminal } from '@/state/terminal';
import { useBrowser } from '@/state/browser';
import { useTunnels } from '@/state/tunnels';
import { useGithubAuth } from '@/state/github';
import GithubSignInDialog from '@/components/github/sign-in-dialog';
import { useUiZoom } from '@/lib/use-ui-zoom';
import { useBreakpoint } from '@/lib/use-breakpoint';
import { IS_WEB } from '@/lib/platform';
import { MobileShell } from '@/components/shell/mobile-shell';
import { TabletShell } from '@/components/shell/tablet-shell';
import { FolderPickerHost } from '@/components/web/folder-picker-host';
import { BrowserWindow } from '@/components/browser/browser-window';

function useActiveProjectSync() {
  const activeProjectId = useExplorer((s) => s.activeProjectId);
  const projects = useExplorer((s) => s.projects);
  const hasLoaded = useExplorer((s) => s.hasLoaded);
  const loadProjects = useExplorer((s) => s.loadProjects);

  useEffect(() => {
    if (!hasLoaded) loadProjects();
  }, [hasLoaded, loadProjects]);

  useEffect(() => {
    const project = projects.find((p) => p.id === activeProjectId);
    if (project) {
      useGit.getState().setActiveProjectId(project.id);
      useAgent.getState().setActiveProject({
        id: project.id,
        name: project.name,
        root: project.root_path,
      });
      return;
    }

    // No project matches the explorer's `activeProjectId`. Before
    // destructively clearing the agent's selection — which deselects the
    // project mid-session and forces sendMessage into a doomed `local-`
    // task — check whether the agent already holds a project that's still
    // present in the list. If so this is a transient explorer desync (a
    // reload race, a momentary empty id), NOT a real deselection: keep the
    // agent's project and just re-point the explorer at it.
    const agentProject = useAgent.getState().activeProject;
    if (agentProject?.id && projects.some((p) => p.id === agentProject.id)) {
      useGit.getState().setActiveProjectId(agentProject.id);
      return;
    }

    // Only clear once projects have actually loaded. During the initial
    // empty-list window `find` always fails, and clearing there would wipe a
    // valid selection on every startup.
    if (!hasLoaded) return;

    useGit.getState().setActiveProjectId('');
    useAgent.getState().setActiveProject({ id: '', name: '', root: '' });
  }, [activeProjectId, projects, hasLoaded]);
}

// Drive the bottom panel's visibility based on whether there are any terminals.
// Only flips visibility on the 0↔1 transition so a user closing the panel via the
// header X stays closed even while sessions remain alive.
function useBottomPanelAutoVisibility() {
  const sessions = useTerminal((s) => s.sessions);
  // Only *user* terminals drive auto-visibility. Agent-spawned terminals are
  // tracked separately in the chat dock and must NOT pop the bottom panel open
  // (the user opens a specific one explicitly when they want it).
  const userTerminalCount = sessions.filter((s) => !s.is_agent).length;
  const prevHadTerminalsRef = useRef(userTerminalCount > 0);
  useEffect(() => {
    const hasTerminals = userTerminalCount > 0;
    if (hasTerminals !== prevHadTerminalsRef.current) {
      useLayout.getState().setBottomPanelVisible(hasTerminals);
      prevHadTerminalsRef.current = hasTerminals;
    }
  }, [userTerminalCount]);
}

// Returns true when the middle column has anything to show — any editor tab
// across any group, or the bottom panel. Drives whether the chat dock docks
// to the right or expands to fill the entire main area.
function useHasMiddleContent() {
  const hasEditorTabs = useEditor((s) =>
    (s.groups ?? []).some((g) => g.tabs.length > 0),
  );
  const bottomPanelVisible = useLayout((s) => s.bottomPanelVisible);
  return hasEditorTabs || bottomPanelVisible;
}

function MiddleColumn({ bottomPanelVisible }) {
  const bottomPanelFullscreen = useLayout((s) => s.bottomPanelFullscreen);

  // In fullscreen mode, show only the terminal panel
  if (bottomPanelFullscreen && bottomPanelVisible) {
    return <BottomPanelHost />;
  }

  return (
    <ResizablePanelGroup direction="vertical">
      <ResizablePanel id="editor" defaultSize={bottomPanelVisible ? '70%' : '100%'}>
        <EditorAreaHost />
      </ResizablePanel>
      {bottomPanelVisible && (
        <>
          <ResizableHandle />
          <ResizablePanel id="bottom" defaultSize="30%" minSize="10%" maxSize="70%">
            <BottomPanelHost />
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  );
}

function MainArea({ chatDockOpen, bottomPanelVisible, hasMiddleContent }) {
  if (!chatDockOpen) {
    return <MiddleColumn bottomPanelVisible={bottomPanelVisible} />;
  }

  // No file open + no terminal: chat swallows the middle column. We render the
  // editor host nowhere in this branch — it has nothing to render and skipping
  // it avoids the empty tab-strip flicker on initial paint.
  if (!hasMiddleContent) {
    return <AgentPanel />;
  }

  // Open file (or terminal) → chat collapses to a right-side dock so the
  // editor / terminal can take the middle. A different panel-group key from
  // the non-dock layout prevents react-resizable-panels from re-using the
  // sibling sizing state across the two structurally distinct trees.
  return (
    <ResizablePanelGroup direction="horizontal" id="chat-dock-main">
      <ResizablePanel id="middle" defaultSize="65%" minSize="30%">
        <MiddleColumn bottomPanelVisible={bottomPanelVisible} />
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel id="chat-dock" defaultSize="35%" minSize="22%" maxSize="60%">
        <AgentPanel />
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

export default function App() {
  const sidebarVisible = useLayout((s) => s.sidebarVisible);
  const bottomPanelVisible = useLayout((s) => s.bottomPanelVisible);
  const chatDockOpen = useLayout((s) => s.chatDockOpen);
  const hasMiddleContent = useHasMiddleContent();
  useActiveProjectSync();
  useBottomPanelAutoVisibility();
  useUiZoom();

  // The Rust backend intercepts CloseRequested, prevents the OS default, and
  // emits this event so the frontend can clean up then call confirm_quit.
  useEffect(() => {
    let unlisten;
    listen('rustic:close-requested', () => {
      invoke('confirm_quit').catch(() => {});
    }).then((fn) => { unlisten = fn; }).catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, []);

  // Bind agent event listeners at app startup. Doing this at the top level —
  // rather than inside AgentPanel / AgentTaskTree effects — guarantees the
  // listeners are alive whenever the backend emits, regardless of which
  // agent UI is currently mounted. bindListeners is a true singleton and
  // returns a no-op cleanup, so the second-arg dep list is irrelevant.
  useEffect(() => { useAgent.getState().bindListeners(); }, []);

  // Resolve the user's GitHub identity once on startup, if a token is already
  // stored. The store guards against re-running, so this is safe to call
  // unconditionally on every mount.
  useEffect(() => { useGithubAuth.getState().init(); }, []);

  // Wire the embedded browser + tunnel hub listeners (web build only). Done at
  // the App level — not inside the desktop ActivityBar — so the browser stays
  // live in the phone/tablet shells, which never mount the activity bar. The
  // store guards (listenersWired) make these calls idempotent.
  useEffect(() => {
    if (!IS_WEB) return;
    useBrowser.getState().wireListeners();
    useBrowser.getState().refreshTabs();
    useTunnels.getState().wire();
  }, []);

  // Probe for the `git` CLI on startup. The Rust backend uses git as a
  // subprocess for state-mutating VCS ops (push/pull/commit/etc.) per the
  // gix migration in docs/educated-guesses/006; without it, every git
  // action will fail with an opaque OS-level error. The check itself is
  // ~30 ms (`git --version`), so we run it eagerly and warn once on miss.
  useEffect(() => {
    invoke('git_check_available')
      .then((res) => {
        if (res && res.available === false && res.message) {
          toast.error(res.message, {
            duration: Infinity,
            // Persistent — user needs to act on this, not have it autoclose.
            // No close button = sonner's default dismiss-on-click behaviour.
            id: 'git-not-available',
          });
        }
      })
      .catch(() => {
        // The command itself failing is non-fatal; subsequent git operations
        // will surface their own errors with the same install-guidance text.
      });
  }, []);

  // Re-register custom fonts that were loaded in previous sessions. localStorage
  // remembers which fonts were loaded and where they're applied, but document.fonts
  // is wiped on every page reload — without this, font-family CSS references point
  // at fonts the browser has never seen, and silently fall back to system fonts.
  useEffect(() => {
    // Migration: the terminal target was removed because xterm requires
    // monospace — drop any leftover terminal mapping so the cleanup is final.
    try {
      const raw = localStorage.getItem('rustic_font_applications');
      if (raw) {
        const apps = JSON.parse(raw);
        if (apps && 'terminal' in apps) {
          delete apps.terminal;
          localStorage.setItem('rustic_font_applications', JSON.stringify(apps));
        }
      }
    } catch { /* ignore */ }

    useSettings.getState().rehydrateFonts().then(() => {
      // Nudge FontBridge / monaco to re-resolve fonts now that the FontFace
      // objects actually exist.
      window.dispatchEvent(new CustomEvent('rustic:font-applied', { detail: { rehydrated: true } }));
    });
  }, []);

  const { isPhone, isTablet } = useBreakpoint();
  const mobile = IS_WEB && (isPhone || isTablet);

  const overlays = (
    <>
      {!IS_WEB && <WindowControls />}
      {IS_WEB && <FolderPickerHost />}
      {IS_WEB && <BrowserWindow />}
      <Toaster />
      <ConfirmDialogHost />
      <CommandPalette />
      <TerminalProjectPicker />
      <ThemeBridge />
      <OnboardingWizard />
      <ShortcutCheatsheet />
      <SettingsModal />
      <KeybindingBridge />
      <FontBridge />
      <GithubSignInDialog />
    </>
  );

  let workbench;
  if (mobile) {
    workbench = isPhone ? <MobileShell /> : <TabletShell />;
  } else {
    workbench = (
      <div className="flex h-full w-full flex-col bg-background text-foreground">
        <ActivityBar />
        <div className="flex flex-1 overflow-hidden">
          <ResizablePanelGroup direction="horizontal" className="flex-1">
            {sidebarVisible && (
              <>
                <ResizablePanel id="sidebar" defaultSize="20%" minSize="12%" maxSize="40%">
                  <SidebarHost />
                </ResizablePanel>
                <ResizableHandle />
              </>
            )}
            <ResizablePanel id="main" defaultSize={sidebarVisible ? '80%' : '100%'}>
              <MainArea
                chatDockOpen={chatDockOpen}
                bottomPanelVisible={bottomPanelVisible}
                hasMiddleContent={hasMiddleContent}
              />
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
        <StatusBar islandToggle={IS_WEB} />
      </div>
    );
  }

  return (
    <TooltipProvider delayDuration={300}>
      {workbench}
      {overlays}
    </TooltipProvider>
  );
}
