import { toast } from 'sonner';

/** Readable message from anything thrown (Error, invoke string, object). */
export function errorText(e) {
  if (e == null) return 'Unknown error';
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message || String(e);
  if (typeof e.message === 'string') return e.message;
  return String(e);
}

/** Show `prefix: <error>` as an error toast. */
export function toastError(prefix, e, opts) {
  return toast.error(prefix ? `${prefix}: ${errorText(e)}` : errorText(e), opts);
}

export default toastError;
