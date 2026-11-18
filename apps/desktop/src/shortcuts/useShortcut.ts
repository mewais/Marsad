import { useEffect, useRef } from "react";
import { registerShortcut, type Shortcut } from "./registry";

/** Register a shortcut for the lifetime of a component. `run`/`when` may close over fresh state. */
export function useShortcut(s: Shortcut) {
  const ref = useRef(s);
  ref.current = s;
  const keysId = s.keys.join(" ");
  useEffect(
    () =>
      registerShortcut({
        id: s.id,
        keys: s.keys,
        title: s.title,
        group: s.group,
        allowInInput: s.allowInInput,
        priority: s.priority,
        run: (e) => ref.current.run(e),
        when: () => ref.current.when?.() ?? true,
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [s.id, keysId, s.title, s.group, s.allowInInput, s.priority],
  );
}
