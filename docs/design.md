# Design

How the app should look, for the design pass after parity. The aim: as close to a good native
Mac app as GPUI lets us get, without paying for it in frames. We don't copy the Swift app 1:1
(no Liquid Glass here); we take its calm and its hierarchy.

## Principles

- **Quiet by default, clear on purpose.** Most of the window is content. Chrome is neutral grey,
  text-coloured or secondary; colour is for state (accent = selected or primary action, orange =
  needs you, red = failed, green = done). One accent per view at most.
- **Hierarchy from size and weight, not boxes.** Fewer borders and cards; use spacing and a
  secondary colour first. A card earns its border when it's interactive or groups unlike things.
- **Icons with labels where space allows, icons alone with tooltips where it doesn't.** Every
  icon-only button has a tooltip, with its shortcut. Icons are Lucide (gpui-kit's
  `IconName`, extra ones in `ui/icons.rs`), 14 pt in rows and toolbars, 16 pt in empty states.
- **Text glyphs are not icons.** Replace ☰ ⌖ ◧ ⎇ ▣ ↳ ✓ and friends with real icons.
- **Consistent controls.** Toolbar actions: ghost buttons, small. Primary action of a view or card:
  one `primary` button. Secondary: `outline`. Destructive: confirm first, red only in the confirm.
- **Hover never moves anything.** Hover shows a background (`theme.hover`) or reveals actions in
  space already reserved; row heights never change.

## Tokens (crates/yh-app/src/theme.rs)

- Type: `text::CAPTION` 11, `FOOTNOTE` 12, `BODY` 13, `CODE` 12, `HEADLINE` 13 semibold, `TITLE`
  15, `LARGE_TITLE` 22. No other sizes. Body line height 19, code 17.
- Fonts: `UI` (SF Pro) and `MONO` (SF Mono, `.AppleSystemUIFontMonospaced`).
- Radii: `radius::SMALL` 5 (chips, inline code), `MEDIUM` 7 (buttons, rows, inputs), `LARGE` 12
  (cards, composer, popovers).
- Spacing: `space::XS` 4, `S` 8, `M` 12, `L` 16, `XL` 24. Rows: 6–8 vertical padding.
- Colours: `text`, `secondary`, `tertiary` for the three text levels; `hover`, `selected`,
  `accent_soft` for backgrounds; `separator` for the few lines left; `success`, `warning`, `error`.

## Areas

- **Sidebar**: rows 28 pt, project headers as small caps-weight secondary labels, selection a soft
  rounded fill (not the full accent unless focused), thread status as a small icon on the left.
- **Thread header**: title (headline), subtitle (footnote, secondary); toolbar icons right-aligned
  with tooltips.
- **Transcript**: your messages in a soft bubble; Claude's replies as plain text on the canvas;
  work rows and tool steps compact and secondary, opening into cards; code blocks with a header
  (language, copy) and SF Mono.
- **Composer**: one rounded card (radius LARGE), shadow-free, a hairline border, controls in a
  bottom row of ghost buttons with icons (paperclip, model, effort, mode), Send as the one primary.
- **Right sidebar**: segmented tab control at the top with icons; panel headers like the thread
  header.
- **Pages (Memory, Usage, Project)**: large title, a subtitle line, actions as a toolbar row; tabs
  as a segmented control, not a row of buttons.
- **Menus** (gpui-component's, patched): the popover surface (a card in dark, white in light), a
  hairline edge, radius LARGE with rows MEDIUM inside 5 pt of padding, one soft shadow; rows 22 pt,
  13 pt text, a 14 pt check/icon column, the lit row the soft accent; section names caption
  semibold secondary; notes (`page_kit::menu_note`) caption tertiary; separators hairlines.
- **Settings, sheets, popovers**: grouped rows like macOS System Settings: section title, rows
  with label left and control right, footers in footnote secondary.

## Performance rules still apply

Shadows, blurs and animations cost frames. No animated spinners in cached views (a still icon),
no shadows on rows, at most one shadow per popover. Check `scripts/perf-run.sh` stays where it
was (docs/performance.md) after the pass.
