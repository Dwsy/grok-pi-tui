---
name: Pi-Grok Configuration Workbench
description: A bilingual workbench for finding, inspecting, editing, and explicitly saving configuration.
colors:
  primary: "#315fc5"
  primary-soft: "#edf2ff"
  on-primary: "#ffffff"
  graphite: "#202731"
  graphite-text: "#eef2f8"
  graphite-active: "#e9effa"
  graphite-active-text: "#233d66"
  canvas: "#f5f6f8"
  surface: "#ffffff"
  surface-subtle: "#f8f9fb"
  surface-hover: "#f0f2f5"
  border: "#d9dde5"
  border-strong: "#bec5d0"
  text: "#1d2430"
  text-muted: "#526174"
  success: "#18794e"
  warning: "#9a6700"
  danger: "#b4232c"
  dark-canvas: "#111318"
  dark-surface: "#181b21"
  dark-border: "#2d333d"
  dark-text: "#edf0f5"
  dark-text-muted: "#a5afbd"
  dark-primary: "#82a9ff"
  dark-primary-soft: "#202c45"
  dark-on-primary: "#101318"
typography:
  body:
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif'
    fontSize: "14px"
    lineHeight: 1.5
  headline:
    fontSize: "30px"
    lineHeight: 1.25
    letterSpacing: "-0.02em"
  title:
    fontSize: "18px"
    lineHeight: 1.3
    letterSpacing: "-0.012em"
  label:
    fontSize: "13px"
    fontWeight: 550
  code:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
rounded:
  sm: "6px"
  surface: "14px"
  dialog: "16px"
spacing:
  sm: "8px"
  md: "16px"
  lg: "24px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    rounded: "{rounded.sm}"
    padding: "7px 12px"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.sm}"
    padding: "7px 12px"
  input:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.sm}"
    padding: "8px 10px"
  surface:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.text}"
    rounded: "{rounded.surface}"
---

# Design System: Pi-Grok Configuration Workbench

## Overview

**Creative North Star: "Configuration Workbench"**

The implemented interface keeps navigation, configuration scope, and pending edits visible. Graphite navigation anchors cool light or dark work surfaces; compact ruled lists give configuration data priority over decoration.

**Key Characteristics:**

- Persistent section navigation and global search.
- Separate current-session and startup-default context.
- Bilingual interface copy and explicit staged saving.
- Flat content surfaces with elevation reserved for overlays.

This document describes the extension's current implementation, not a new design proposal. Sources are `web/index.html`, `web/styles.css`, the JavaScript renderers, `web/i18n.json`, and `web/ui-config.json`. No production imagery or generated assets were introduced.

## Colors

Primary blue identifies actions, selected controls, focus, and pending-save state. Graphite navigation retains its dark identity in both themes. Neutral canvas, surface, hover, border, and text roles are CSS custom properties; the dark theme overrides these roles rather than changing component structure. System preference applies unless an explicit theme is selected.

Success, warning, and danger use paired text and tinted backgrounds. State text accompanies color.

**The State Color Rule.** Use color to reinforce a named state or action; retain the label and visible focus treatment.

## Typography

System UI type supports both English and Chinese without downloaded fonts. Headings establish a compact hierarchy; body and helper copy stay readable at configuration density. Code, paths, identifiers, and raw JSON use the system monospace stack. The small π brand mark is the sole Georgia/serif treatment.

At the mobile breakpoint, the main heading reduces to (26px). Text wraps for long values where necessary; truncated list labels retain the underlying user value.

## Layout

Desktop uses a sticky full-height sidebar (244px) and fluid content, with panels capped at (1400px). The main area has (36px) horizontal padding. A session/default strip precedes all four sections.

Models use a provider column (210–260px), a detail column, and a (22px) gap. Resource and model entries use ruled rows; settings group related labeled controls. Sticky save bars keep pending state and Save/Discard available.

At (1100px), model actions and setting controls stack and main padding contracts. At (980px), quick settings use one column. At (760px), the shell becomes a single column: navigation becomes four equal icon-and-label items, sidebar metadata hides, provider details stack below a scrollable list, and fields/dialogs collapse. Mobile content uses (16px) horizontal padding. The document minimum width is (320px).

## Elevation & Depth

Content surfaces rely on tone and one-pixel borders. Shadows are reserved for dialogs and global status feedback: dialogs use (0 24px 80px rgb(0 0 0 / .24)); status feedback uses (0 10px 30px rgb(0 0 0 / .14)). A dark translucent backdrop separates modal work.

Dialogs reveal over (180ms) with a slight vertical movement and clipping. Switch feedback uses (120ms). Reduced-motion preferences disable animations and transitions.

## Shapes

Controls have gently curved corners; larger surfaces and dialogs use the frontmatter's larger radii. Badges use a compact (5px) radius. Save bars use (12px). Repeated list rows remain flat and divided, avoiding nested card borders.

## Components

- **Navigation:** semantic `nav` with four buttons for Models, Resources, Grok/F2, and Pi settings. The active item uses `aria-current="page"`, a pale selection fill, and dark text; inline SVG icons reinforce labels.
- **Search:** sidebar trigger and Cmd/Ctrl+K open a native dialog that searches across sections and routes a chosen result to its setting or resource. Local section filters remain available.
- **Session/default strip:** clearly distinguishes the current model from persisted startup defaults; these are different operations.
- **Buttons and inputs:** primary blue for the save/create action, bordered neutral secondary actions, quiet sidebar actions, and danger text for destructive actions. Keyboard focus uses a visible two-pixel outline; disabled controls are dimmed.
- **Staged saving:** Pi settings and Grok/F2 values have separate drafts, pending-state save bars, Save and Discard. Quick settings and raw JSON participate in the Pi draft. Save operations show busy feedback and prevent overlapping writes. Do not imply that all provider/resource operations use these drafts.
- **Settings metadata:** `web/ui-config.json` owns interface configuration and static field metadata. Grok catalog rendering merges static entries with the runtime host catalog by key, with runtime metadata taking precedence. Repository configuration contracts remain authoritative; undocumented capabilities must not be inferred from visual controls.
- **Localization:** `web/i18n.json` supplies English and Chinese UI strings, including accessible labels. User resource descriptions, paths, identifiers, model/provider names, and raw configuration data retain their original values. Runtime metadata can supply fallback labels.
- **Feedback and access:** skip link, labeled sections and fields, native dialogs, live status text, empty/error states, and visible focus provide the operational scaffolding. Global status appears at the upper right.

## Do's and Don'ts

### Do

- Do preserve the graphite navigation and role-based light/dark content colors.
- Do keep current-session actions distinct from persisted defaults.
- Do preserve explicit Save/Discard and pending-state feedback for Pi and Grok settings.
- Do use configuration metadata and translation keys for supported interface controls.

### Don't

- Don't translate user-authored data or configuration identifiers.
- Don't present static UI metadata as proof of unsupported backend capabilities.
- Don't add decorative imagery or downloaded fonts to this implemented system without an explicit design change.
- Don't claim rendered visual verification from this source-based documentation pass.
