// Desktop notifications (notifications.rs, Linux) → the island. They arrive as
// one "Notifications" pill: each one is a step on it, the island opens its card
// with the newest, and the pill you were on comes back a few seconds later.

import { Bridge, onEvent } from "../core/bridge";
import { toOneLine } from "../core/diff";
import { Sound } from "../core/sound";
import { State, type AgentTask } from "../core/state";
import { t } from "../i18n/i18n";
import type { Island } from "./island";

export const NOTIFICATIONS_ID = "agent_notifications";
const COLOR = "#5AC8FA";
/** How long the pill stays on screen before the previous one returns. */
const SHOWN_MS = 8000;
/** A new version waits for a look: the card stays open this long (the island stays pinned). */
const UPDATE_MS = 120_000;

interface DesktopNotification {
  app: string;
  summary: string;
  body: string;
  target?: string | null;
}

/** The notification card's button: the app that sent the newest one. */
export function openNoticeTarget(task: AgentTask | null | undefined): boolean {
  if (task?.id !== NOTIFICATIONS_ID || !task.noticeTarget) return false;
  void Bridge.openNotificationTarget(task.noticeTarget);
  return true;
}

let restoreTimer: number | null = null;
let before: string | null = null;
let updatePinHeld = false;
// Windows may republish the same toast with a new arrival timestamp.
const recentNotices = new Map<string, number>();
const DEDUPE_MS = 20_000;

export function isRepeatedNotice(n: DesktopNotification, now = Date.now()): boolean {
  const key = JSON.stringify([n.app, n.summary, n.body, n.target ?? ""]);
  for (const [oldKey, time] of recentNotices) {
    if (now - time >= DEDUPE_MS) recentNotices.delete(oldKey);
  }
  const last = recentNotices.get(key);
  if (last !== undefined && now - last < DEDUPE_MS) return true;
  recentNotices.set(key, now);
  return false;
}

/** "Discord: Anna — hey!" on one line. */
export function noticeLine(n: DesktopNotification): string {
  const head = n.app ? `${n.app}: ` : "";
  const opaque = n.summary.length >= 24 && /^[a-zA-Z0-9_-]+$/.test(n.summary);
  const summary = opaque && n.body ? "" : n.summary;
  const text = summary && n.body ? `${summary} — ${n.body}` : summary || n.body;
  return toOneLine(`${head}${text}`);
}

export function registerNotificationHandlers(island: Island) {
  void onEvent<DesktopNotification>("desktop-notification", (n) => handle(island, n));
}

function handle(island: Island, n: DesktopNotification) {
  // Coucou's own notices (a new version) show even with the mirroring off.
  // Song changes belong to the Spotify pill, not desktop notifications.
  if (n.app.trim().toLowerCase() === "spotify") return;
  if (State.paused || (!State.settings.showNotifications && n.app !== "Coucou")) return;
  const line = noticeLine(n);
  if (!line || isRepeatedNotice(n)) return;
  const update = n.app === "Coucou" && !!n.target;
  State.upsertExternalAgent(NOTIFICATIONS_ID, t("Notifications"), COLOR);
  State.appendStep(NOTIFICATIONS_ID, line);
  const task = State.tasks.find((t) => t.id === NOTIFICATIONS_ID);
  if (task) {
    task.finalLine = line;
    task.noticeApp = n.app || null;
    task.noticeTarget = n.target || null;
  }
  State.updateTask(NOTIFICATIONS_ID, "finished");

  // A card waiting for an answer is never covered: the badge says it came.
  if (State.pendingApproval) {
    State.setPillBadge(NOTIFICATIONS_ID, "finished");
    return;
  }
  if (State.focusId !== NOTIFICATIONS_ID) before = State.focusId;
  State.setFocus(NOTIFICATIONS_ID);
  Sound.play("peek");
  // The compact island has no text: the card with the message opens, as it does
  // when Claude Code finishes.
  if (State.mode !== "expanded") island.alert("finished");
  if (update) {
    island.stay();
    updatePinHeld = true;
  }

  if (restoreTimer != null) window.clearTimeout(restoreTimer);
  restoreTimer = window.setTimeout(() => {
    restoreTimer = null;
    if (updatePinHeld) {
      island.dropPin();
      updatePinHeld = false;
    }
    State.updateTask(NOTIFICATIONS_ID, "idle");
    if (State.focusId === NOTIFICATIONS_ID && before && State.tasks.some((t) => t.id === before)) {
      State.setFocus(before);
    }
  }, update ? UPDATE_MS : SHOWN_MS);
}
