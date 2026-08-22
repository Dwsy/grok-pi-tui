/**
 * Shared constants and wire types for the experimental Remote TUI.
 *
 * Widget keys and the meta filename are part of the adapter/Pager contract
 * (`pi_adapter.rs`, `acp_handler/mod.rs`) — rename only in lockstep.
 */

import type { Component } from "@earendil-works/pi-tui";
import type { FSWatcher } from "node:fs";

export const WIDGET_KEY = "remote_tui";
export const LAYOUT_WIDGET_KEY = "__pi_grok_remote_tui_layout__";
export const META_NAME = "pi-grok-remote-tui-active.json";

export type UnknownRecord = Record<string, unknown>;

export type RemoteTuiLayout = {
  overlay: boolean;
  /** Width passed to Component.render(), matching Pi TUI overlay sizing. */
  width: number;
  maxHeight?: number | string;
  anchor?: string;
  row?: number | string;
  col?: number | string;
  offsetX?: number;
  offsetY?: number;
};

export type ComponentLike = Component & { dispose?(): void };

export type RemoteTuiDemoUi = {
  setWidget: (key: string, lines: string[] | undefined, options?: { placement?: string }) => void;
  setStatus?: (key: string, text?: string) => void;
  setTitle?: (title: string) => void;
  setEditorText?: (text: string) => void;
};

export type ActiveHost = {
  id: string;
  component: ComponentLike;
  closed: boolean;
  width: number;
  keysPath: string;
  watcher: FSWatcher | null;
  keyOffset: number;
  close: (result: unknown) => void;
  pushFrame: () => void;
  handleInput: (data: string) => void;
};
