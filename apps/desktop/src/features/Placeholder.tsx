import { PageHeader } from "../shell/PageHeader";
import { useNavigate } from "../router";
import { Button, EmptyState, type IconName } from "../ui";

/** Unknown route. */
export function Placeholder({ title, icon }: { title: string; icon: IconName }) {
  const nav = useNavigate();
  return (
    <>
      <PageHeader title={title} />
      <div className="placeholder">
        <EmptyState icon={icon} title="Nothing here" action={<Button size="sm" onClick={() => nav("/inbox")}>Go to the Inbox</Button>}>This address does not match a screen.</EmptyState>
      </div>
    </>
  );
}
