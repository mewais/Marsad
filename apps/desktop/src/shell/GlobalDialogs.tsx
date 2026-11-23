import { lazy, Suspense } from "react";
import { useStore } from "../lib/store";
import { NewSessionDialog } from "../features/newsession/NewSessionDialog";
import { SignInDialogs } from "../features/hosts/AuthPromptDialog";
import { closeDialogs, dialogStore } from "./dialogs";

const NewWorkstreamDialog = lazy(() => import("../features/workstreams/NewWorkstreamDialog").then((m) => ({ default: m.NewWorkstreamDialog })));

/** Dialogs any screen, shortcut or palette action can open. */
export function GlobalDialogs() {
  const d = useStore(dialogStore, (s) => s);
  return (
    <>
      <SignInDialogs />
      <NewSessionDialog open={!!d.newSession} preset={d.newSession} onClose={closeDialogs} />
      {d.newWorkstream ? (
        <Suspense fallback={null}>
          <NewWorkstreamDialog open onClose={closeDialogs} />
        </Suspense>
      ) : null}
    </>
  );
}
