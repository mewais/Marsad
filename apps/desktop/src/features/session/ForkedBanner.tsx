import type { SessionSnapshot } from "../../bridge/wire";
import { Link, sessionPath } from "../../router";
import { useSessionView } from "../../state";
import { Icon } from "../../ui";

/**
 * A forked session says where it came from, with a link to the parent. The parent is named by its title when it is in the
 * fleet; a parent the app does not know (deleted, or on a host that has not reported) is named by its id.
 */
export function ForkedBanner({ host, snap }: { host: string; snap: SessionSnapshot }) {
  const from = snap.forked_from;
  if (!from) return null;
  return <ForkedFromLine host={host} parentId={from.session} />;
}

function ForkedFromLine({ host, parentId }: { host: string; parentId: string }) {
  const parent = useSessionView(`${host}/${parentId}`);
  return (
    <div className="banner info sd-forked" role="status" data-testid="forked-from">
      <Icon name="branch" size="sm" />
      <div className="bt">
        <span>Forked from <b>{parent?.name ?? parentId}</b></span> · <Link to={sessionPath(host, parentId)}>open</Link>
      </div>
    </div>
  );
}
