import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./Button";
import { EmptyState } from "./Misc";
import { Icon } from "./Icon";

export interface ErrorBoundaryProps {
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

interface State {
  error: Error | null;
}

/**
 * Catches an exception thrown while drawing `children` and shows what failed and how to reload
 * just that part, instead of letting React unmount the whole window. The error itself is shown
 * (it is the only fact the user has to report) and written to the console.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`Could not draw ${this.props.what}:`, error, info.componentStack);
  }

  componentDidUpdate(prev: ErrorBoundaryProps) {
    if (this.state.error && !Object.is(prev.resetKey, this.props.resetKey)) this.setState({ error: null });
  }

  private reset = () => this.setState({ error: null });

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
