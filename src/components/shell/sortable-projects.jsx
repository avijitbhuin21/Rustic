import React from 'react';
import { GripVertical } from 'lucide-react';
import {
  DndContext,
  PointerSensor,
  useSensor,
  useSensors,
  closestCenter,
} from '@dnd-kit/core';
import {
  restrictToVerticalAxis,
  restrictToParentElement,
} from '@dnd-kit/modifiers';
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
  arrayMove,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { toast } from 'sonner';
import { useExplorer } from '@/state/explorer';
import { cn } from '@/lib/utils';

/**
 * DnD wrapper for a vertical list of project sections. Reordering is persisted
 * through the shared explorer store so every panel (Explorer, Source Control,
 * Agent) reflects the same order.
 */
export function SortableProjectList({ projects, children }) {
  const reorderProjects = useExplorer((s) => s.reorderProjects);

  // Require a small drag distance before a pointer-down on the grip becomes a
  // drag, so a plain click on the handle still toggles the project.
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } })
  );

  const onDragEnd = (event) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const ids = projects.map((p) => p.id);
    const oldIndex = ids.indexOf(active.id);
    const newIndex = ids.indexOf(over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    reorderProjects(arrayMove(ids, oldIndex, newIndex)).catch((err) =>
      toast.error(`Reorder failed: ${err?.message || err}`)
    );
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      modifiers={[restrictToVerticalAxis, restrictToParentElement]}
      onDragEnd={onDragEnd}
    >
      <SortableContext
        items={projects.map((p) => p.id)}
        strategy={verticalListSortingStrategy}
      >
        {children}
      </SortableContext>
    </DndContext>
  );
}

/**
 * useSortable wrapper returning the ref, transform style, and drag-handle props
 * a project section header needs to become draggable.
 */
export function useProjectSortable(projectId) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: projectId });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 10 : undefined,
    opacity: isDragging ? 0.6 : 1,
  };

  return { setNodeRef, style, dragHandleProps: { ...attributes, ...listeners } };
}

/**
 * Grip handle button for a project header. Reveals on hover of a parent marked
 * with the `group/project` class; dragging starts only from here so clicking
 * the header still toggles it.
 */
export function ProjectDragHandle({ dragHandleProps, className }) {
  return (
    <button
      {...dragHandleProps}
      onClick={(e) => e.stopPropagation()}
      className={cn(
        '-ml-1 flex size-4 shrink-0 cursor-grab touch-none items-center justify-center text-muted-foreground/50 opacity-0 hover:text-foreground focus-visible:opacity-100 active:cursor-grabbing group-hover/project:opacity-100',
        className
      )}
      title="Drag to reorder project"
      aria-label="Drag to reorder project"
    >
      <GripVertical className="size-3" />
    </button>
  );
}
