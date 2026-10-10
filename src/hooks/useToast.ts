import { useCallback, useEffect, useRef, useState } from "react";

/** A transient message: `flash(msg)` shows it for a few seconds. */
export function useToast(ms = 3200): [string | null, (msg: string) => void] {
  const [toast, setToast] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  const flash = useCallback(
    (msg: string) => {
      setToast(msg);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setToast(null), ms);
    },
    [ms]
  );

  useEffect(() => () => clearTimeout(timer.current), []);
  return [toast, flash];
}
