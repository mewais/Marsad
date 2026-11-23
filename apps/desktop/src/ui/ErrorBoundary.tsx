import { Component, createContext, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./Button";
import { EmptyState } from "./Misc";
import { Icon } from "./Icon";

/**
 * Where a boundary reports that it caught a render failure, given the boundary's `name` and
 * nothing else (the app provides it: the failure is recorded in its diagnostics log). Without a
 * provider (a bare tree in a test or a preview) nothing is reported.
 */
export const RenderErrorReport = createContext<(component: string) => void>(() => undefined);

export interface ErrorBoundaryProps {
  /**
   * Which boundary this is, as a machine word (letters, digits and `_ . : -`: `page.config`,
   * `transcript_entry`). It is what a render failure is reported under; the error's message and the
   * props are never reported.
   */
  name: string;
  /**
   * How much failed: `app` (the whole window: reload it), `page` (the routed page: reload that page),
   * `item` (one row of a list: the rest of the list stays).
   */
  variant: "app" | "page" | "item";
  /** What was being drawn, in the user's words ("this page", "this message"). */
  what: string;
  /** Another way out of a failed page (e.g. back to the Inbox). */
  secondary?: { label: string; run: () => void };
  /** When this changes (another page, a newer version of the item) the boundary tries drawing again. */
  resetKey?: unknown;
  children: ReactNode;
}

/** `State.failedKey` before the failing render's props have been seen. */
const UNSEEN = Symbol("unseen");

interface State {
  error: Error | null;
  /** The `resetKey` the failure happened under: only another key tries drawing again, not the one that just failed. */
  failedKey: unknown;
}

/**
 * Catches an exception thrown while drawing `children` and shows what failed and how to reload
 * just that part, instead of letting React unmount the whole window. The error itself is shown
 * (it is the only fact the user has to report) and written to the console, and the failure is reported
 * once, by the boundary's `name`, through `RenderErrorReport`.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, State> {
  static contextType = RenderErrorReport;
  declare context: React.ContextType<typeof RenderErrorReport>;
  state: State = { error: null, failedKey: UNSEEN };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)), failedKey: UNSEEN };
  }

  /** Learns the key the failure happened under, and draws again once the key is another one. */
  static getDerivedStateFromProps(props: ErrorBoundaryProps, state: State): Partial<State> | null {
    if (!state.error) return null;
    if (state.failedKey === UNSEEN) return { failedKey: props.resetKey };
    return Object.is(state.failedKey, props.resetKey) ? null : { error: null, failedKey: UNSEEN };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`Could not draw ${this.props.what}:`, error, info.componentStack);
    this.context(this.props.name);
  }

  private reset = () => this.setState({ error: null, failedKey: UNSEEN });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const { variant, what, secondary } = this.props;
    const message = error.message || String(error);
    if (variant === "item")
      return (
        <div className="boundary-item c-failed fs12" role="alert" data-testid="boundary-item">
          <Icon name="warn" size="xs" /> Could not display {what}: <span className="mono">{message}</span>{" "}
          <button type="button" className="linkbtn" onClick={this.reset}>Try again</button>
        </div>
      );
    return (
      <div className={variant === "app" ? "boundary-app" : "placeholder"} data-testid={`boundary-${variant}`}>
        <EmptyState
          icon="failed"
          title={variant === "app" ? "Marsad hit an error and cannot show this window" : `Could not display ${what}`}
          action={
            <div className="boundary-acts">
              {variant === "app" ? <Button size="sm" variant="primary" icon="refresh" onClick={() => window.location.reload()}>Reload the window</Button> : <Button size="sm" variant="primary" icon="refresh" onClick={this.reset}>Reload {what}</Button>}
              {secondary ? <Button size="sm" onClick={secondary.run}>{secondary.label}</Button> : null}
            </div>
          }
        >
          <span className="mono">{message}</span>
          <br />
          {variant === "app" ? "Your sessions keep running; nothing was lost on the machines." : "The rest of Marsad keeps working."}
        </EmptyState>
      </div>
    );
  }
}
