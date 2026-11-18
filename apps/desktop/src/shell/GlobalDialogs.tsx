import { lazy, Suspense } from "react";
import { useStore } from "../lib/store";
import { NewSessionDialog } from "../features/newsession/NewSessionDialog";
import { AuthPromptDialog, useAuthPrompt } from "../features/hosts/AuthPromptDialog";
import { closeDialogs, dialogStore } from "./dialogs";

const NewWorkstreamDialog = lazy(() => import("../features/workstreams/NewWorkstreamDialog").then((m) => ({ default: m.NewWorkstreamDialog })));

/** Dialogs any screen, shortcut or palette action can open. */
export function GlobalDialogs() {
  const d = useStore(dialogStore, (s) => s);
  const auth = useAuthPrompt();
  return (
    <>
      {auth ? <AuthPromptDialog key={`${auth.prompt.host}:${auth.prompt.id}`} prompt={auth.prompt} hostName={auth.hostName} /> : null}
      <NewSessionDialog open={!!d.newSession} preset={d.newSession} onClose={closeDialogs} />
      {d.newWorkstream ? (
        <Suspense fallback={null}>
          <NewWorkstreamDialog open onClose={closeDialogs} />
        </Suspense>
      ) : null}
    </>
  );
}
