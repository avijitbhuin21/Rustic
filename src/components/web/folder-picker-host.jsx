import React, { useEffect, useRef, useState } from 'react';
import { setDialogHandler } from '@/lib/web/dialog-bridge';
import { FolderPicker } from './folder-picker';

/** Mounts the folder picker and wires it to the web dialog bridge (web build only). */
export function FolderPickerHost() {
  const [state, setState] = useState({ open: false, options: null });
  const resolveRef = useRef(null);

  useEffect(() => {
    return setDialogHandler(
      (options) =>
        new Promise((resolve) => {
          resolveRef.current = resolve;
          setState({ open: true, options: options || {} });
        }),
    );
  }, []);

  const onResolve = (value) => {
    const resolve = resolveRef.current;
    resolveRef.current = null;
    setState({ open: false, options: null });
    if (resolve) resolve(value);
  };

  return <FolderPicker open={state.open} options={state.options} onResolve={onResolve} />;
}
