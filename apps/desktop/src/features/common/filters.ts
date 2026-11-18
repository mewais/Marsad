import { createStore } from "../../lib/store";
import { NO_FILTERS, type Filters } from "../../state";

/** Filters shared by the Inbox (and later views) so they survive navigation. */
export const filterStore = createStore<Filters>(NO_FILTERS);
export const setFilters = (p: Partial<Filters>) => filterStore.set((f) => ({ ...f, ...p }));
export const resetFilters = () => filterStore.set(NO_FILTERS);
export const filtersActive = (f: Filters) => f.q !== "" || f.status !== "all" || f.driver !== "any" || f.hosts !== "all" || f.workstream !== "any";
