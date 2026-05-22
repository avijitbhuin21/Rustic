import React, { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { TooltipProvider } from '@/components/ui/tooltip';
import { Toaster } from '@/components/ui/sonner';
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from '@/components/ui/resizable';
import { ConfirmDialogHost } from '@/components/confirm-dialog';
import { CommandPalette } from '@/components/command-palette';
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
import { useLayout } from '@/state/layout';
import { useExplorer } from '@/state/explorer';
import { useGit } from '@/state/git';
import { useAgent } from '@/state/agent';
import { useSettings } from '@/state/settings';
import { useUiZoom } from '@/lib/use-ui-zoom';

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
    } else {
      useGit.getState().setActiveProjectId('');
      useAgent.getState().setActiveProject({ id: '', name: '', root: '' });
    }
  }, [activeProjectId, projects]);
}

export default function App() {
  const sidebarVisible = useLayout((s) => s.sidebarVisible);
  const bottomPanelVisible = useLayout((s) => s.bottomPanelVisible);
  useActiveProjectSync();
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

  return (
    <TooltipProvider delayDuration={300}>
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
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
        <StatusBar />
      </div>
      <WindowControls />
      <Toaster />
      <ConfirmDialogHost />
      <CommandPalette />
      <ThemeBridge />
      <OnboardingWizard />
      <ShortcutCheatsheet />
      <SettingsModal />
      <KeybindingBridge />
      <FontBridge />
    </TooltipProvider>
  );
}
