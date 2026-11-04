# DockerVault UI foundation

## Ownership and updates

DockerVault's frontend maintainers own this directory and the compatibility callers.
`button.tsx`, `input.tsx` and `dialog.tsx` adapt the MIT-licensed shadcn/ui **base-nova** registry sources, inspected with shadcn **4.23.0** available. They are copied source, **not** automatically updated by npm. Sources: [button](https://ui.shadcn.com/r/styles/base-nova/button.json), [input](https://ui.shadcn.com/r/styles/base-nova/input.json), [dialog](https://ui.shadcn.com/r/styles/base-nova/dialog.json). Local differences are the existing dark-first tokens, 44px actions, bounded scrolling, compatible imperative callers and focus fallback. The notice is in `SHADCN-LICENSE.txt`.

The single primitive family is **@base-ui/react 1.9.0**, pinned in package.json and the lockfile; class-variance-authority is **0.7.1**. No Radix/Mantine/MUI family is installed. React 19, StrictMode, Router, Query, Zustand, Vite, TypeScript and the existing Tailwind 4/PostCSS pipeline are retained. No generator was run over application code or CSS. Native checkbox/radio/select controls remain valid for simple existing callers; they do not constitute a second primitive package.

To update: inspect the upstream component and Base UI release notes, compare source manually, retain local caller signatures/tokens, pin the reviewed published version and regenerate the npm lock with the repository's Node/npm versions. Do not run `shadcn add --overwrite`. Run locked install, lint, typecheck, full coverage tests and production build, then real StrictMode browser tests for forms, portals, nested Escape, keyboard/focus, mobile navigation, reflow and axe. Keep license notices. Adding a component means extending this catalogue, not copying a sibling into a domain page.

## Catalogue

- `Button`, `IconButton`: Base UI button composition; explicit names for icon actions. Pending/disabled belongs to the mutation owner.
- `Input`, `TextField`: local shadcn input; unique label/control IDs, associated help/error, aria-invalid and announced error. Use native labelled checkbox/radio/switch for simple boolean/single-choice fields; use `role="switch"` with `aria-checked` when using a button.
- `ChoiceSelect`, `SearchSelect`: Base UI select and filterable combobox, stable string values, labels and portalled lists. A combobox is not free-form search. Keep domain number/empty conversions at the caller.
- `Dialog`, `DialogTrigger`, `DialogContent`, `DialogTitle`, `DialogDescription`, `DialogClose`: shadcn-style composition. A positioned DialogContent supplies a sheet; no second drawer stack.
- `Modal`: compatibility composition for imperative existing editors/explorers. Requires a meaningful title and close callback. This is the same Base UI dialog, not another overlay implementation.
- `Menu`, `MenuTrigger`, `MenuContent`, `MenuItem`: Base UI action menu with arrows, typeahead, Escape and trigger return; actions stay with the caller.
- `PageHeader`, `FilterBar`, `EntityTable`, `Pagination`, `SelectionToolbar`, `StatusLabel`, `QueryState`: semantic compositions, not domain querying/pagination/state engines. `EntityTable` requires real thead/tbody and provides a caption plus named, keyboard-scrollable wide-data region. Pagination consumes authoritative page counts. QueryState distinguishes initial loading, empty, filtered empty and error; callers must never turn a failed query into empty.
- Existing `EmptyState`, `LoadingSkeleton` and `ConfirmDialog` remain compatibility entrypoints. Do not create parallel copies when migrating a page.

## Interaction contract

Base UI owns one portal/focus stack for dialogs, sheets, menus, selects and comboboxes. Modal background is unavailable to focus/pointer/assistive navigation, focus enters the popup and returns to its prior trigger; if a completed action removes that element, main content is the fallback. Escape dismisses only the upper eligible surface. Portals use a common 100/110 layer pair; domain content must not create a fixed-position sibling overlay. Keep nested confirmation roots within the parent's React subtree, even when DOM content is portalled.

`pending` blocks outside/Escape dismissal **without cancelling backend work**; rejected Base UI open changes must call `details.cancel()`. Caller cancel/close controls must also be disabled while a non-cancellable submission is pending. The shared busy popup focuses a visible status so keyboard focus remains contained even when every action is disabled. Do not close a confirmation before the mutation resolves: close on success, preserve the dialog and a readable error for retry/cancel on failure. The private-key custody dialog deliberately cannot dismiss before its existing download/confirmation gate; that policy is owned by #181, not a generic cancel operation.

Controlled dialogs capture their interaction origin on the opening render, before a launching menu is removed; a menu item's logical return target is its labelled trigger, not the ephemeral item. Both imperative `Modal` and `ConfirmDialog` use this same policy. Select/combobox popups use the anchor width capped by Base UI's available viewport width, wrap unbroken option names and expose keyboard-scrollable combobox content.

Global navigation/help shortcuts suspend while a shared popup exists (including closing portals), and while typing/modifier keys are active. Wizard Enter/Space no longer advances globally: the focused native control owns activation. Form submission is explicit and duplicate submission is disabled by pending state.

Use visible status text plus an icon, not color alone. Main route changes update title/breadcrumb and focus main; dialogs return focus independently. Skip navigation targets main. CSS tokens provide visible focus, dark-first surfaces, explicit dark variants regardless of system preference and reduced-motion alternatives without removing information.

## Migration and evidence boundaries

The route hierarchy is Overview `/`, Protection `/protection`, policies `/protection/schedules` and `/protection/retention`, History & Restore `/history`, Storage `/storage`, Settings `/settings`. Legacy `/backups`, `/schedules`, `/retention` deliberately redirect. Protection and History temporarily reuse the working backup/configuration/history view and explicitly say so; this does not complete #184–#190. Unimplemented resource/detail addresses render a not-found page, never Overview.

Covered dialog callers: confirmation (including storage nested deletes), backup create/edit, archive browser, logs, restore loading/error/editor, storage browser/editor, schedule/policy forms, encryption custody/instructions/regeneration, SSH generation/installation, notifications, shortcuts and mobile navigation. Keep caller regression tests when migrating these to dedicated pages.

Automated axe/DOM and Chromium keyboard checks do not prove screenreader or physical-device usability. Actual screenreader/device acceptance remains with the redesign owner before #182/#191 closure. Never describe this library integration as WCAG certification.
