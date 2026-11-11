# Windows slow-laptop handoff

[Windows development](windows.md)

**Request:** whoever owns Windows fixture reliability on the shared-session pool, please take the items below.
They come from a ThinkPad P43s (Intel UHD 620, 8 GB). A Release build there takes 20–40 minutes, so that laptop
now only debugs problems that reproduce only on slow hardware. Delete this file when the items are resolved.

## 1. Stack-divider fix: branch `handoff-stack-divider` (61d4935cb)

On short windows such as a 1080p laptop, the divider between two flexible stacked panels (Paint's
Properties/Layers, Brushes/Tool settings) did not move. When minimum heights fit but preferred heights do not,
`resolve_node` (`crates/layer-ui/src/layout.rs`) split the free space by each side's preferred-minus-minimum range
and ignored the dragged fraction. The commit deletes that special case and keeps the existing fraction tiers.
Content-fitted groups (Paint's Color and Navigator) still resolve through `fitted_split`, which is unchanged.

Verified on the laptop: the new layer-ui regression fails before (Properties moved 0 for a −40 pointer delta) and
passes after; the fitted Color/Navigator heights guard passes; full layer-ui 2424 passed. The user approved the
behaviour.

Please validate on the pool and push it:

- Run `panel-bridge-layout` (default, dark) at a display that squeezes the right column, e.g. 1920×1080 at 150 %.
  `622a1fef6` fits fixture windows to the work area. Also run `column-stacks` and `workspace-layout`.
- Open question, line 112 ("Stack-divider samples include changing unacknowledged bridge revisions"). On the laptop,
  the XAML groups moved ±35 DIP during the drag, but the GPU bridge chrome barely followed: dark stayed at revision 13
  for the whole 3 s drag, light went 13→16 with no unacknowledged sample, and the dark composed frames at opposite
  extremes were pixel-identical. Before this fix the divider never moved there, so this path had never run on the
  laptop. Check whether the chrome follows a stack-divider drag on the pool at the squeezed size.
  - If it doesn't, compare with the affected-views family reverted (`d7b6a0fb6`, `97225bc64`, `f8e513d48`,
    `a99e7b7d2`, `67db62758`), fix the cause and push it with the divider commit.
  - If it does, note that here; the laptop will treat it as a slow-hardware problem.

## 2. Unclassified fixture failures from the laptop

These are first attempts with the local private-desktop runner at `3a98291d7` plus the divider patch. None has a
matched control yet.

| Selector | First error |
| --- | --- |
| toolbar-components | pen drag did not lower strength (possible stale native presentation after `d7b6a0fb6`) |
| canvas-editing:smoke-light | Cancel did not restore selection artwork: move preview and cancel |
| persistence | Close exceeded five seconds after authorization |
| scopes | property-action-calibrate-white is clipped outside any scroll container |
| activation | SetFocus: Target element cannot receive focus |
| workspace-pickup, column-stacks:scroll-light | Windows did not move the mouse to the requested point |

Also unclassified, from the `a99e7b7d2` validation: `idle-brush` (pointer placement), `enclose-fill`
(`tool-choice-region-edges-2` missing; the native trace shows the controls exist below the panel viewport) and
`expansion` (viewport acknowledgement). These skipped-presentation handoff rows were never run:
`liquify-pixels`, `photo-tools:transform-dark`, `tool-bubbles:default`, `tool-bubbles:RapidOnly` (the folding-card
tap failed on the candidate and passed on a `17d381a1b` control), `tool-bubbles:SlidersOnly`, `vector-edges` and
`vector-layers:light`.

## 3. Fixture-collector fix: branch `handoff-fixture-cim` (c0bfa6a38)

It ignores retired fixture processes that CIM still lists, with a routing regression that fails before. It wasn't
pushed because runner changes need the full matrix. Run it on the pool, or drop it if recent runner work supersedes it.

## Kept on the laptop

- Intel UHD 620 timing: the untraced UI-thread cost of panel reflow and glass publication during divider drags.
- The bridge-chrome lag, if the pool can't reproduce it.
- Intel UMA-only failures, such as the transform "Source working set exceeds the decoded tile budget" refusal.
