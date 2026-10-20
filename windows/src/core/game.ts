import { Bridge } from "./bridge";
import { State } from "./state";
import { t } from "../i18n/i18n";

let pending = false;

/** Process-based session time survives alt-tab and a late Coucou launch. */
export function gameSessionMinutes(data: Record<string, unknown>, now = Date.now()): number | null {
  const started = data.startedAtMs;
  if (data.playing !== true || typeof started !== "number" || !Number.isFinite(started) || started <= 0 || !Number.isFinite(now)) return null;
  return Math.floor(Math.max(0, now - started) / 60_000);
}

export function gameSessionTime(data: Record<string, unknown>, now = Date.now()): string | null {
  const minutes = gameSessionMinutes(data, now);
  if (minutes === null) return null;
  return minutes < 60 ? t("{n}m", { n: minutes })
    : `${t("{n}h", { n: Math.floor(minutes / 60) })} ${t("{n}m", { n: minutes % 60 })}`;
}

export function nextGameMinuteDelay(data: Record<string, unknown>, now = Date.now()): number | null {
  if (gameSessionMinutes(data, now) === null) return null;
  return 60_000 - (Math.max(0, now - (data.startedAtMs as number)) % 60_000);
}

export async function focusGame(): Promise<void> {
  if (pending) return;
  pending = true;
  const id = "integration_game";
  const updateError = (error: string | null) => {
    const info = State.integrations[id];
    if (info) State.integrations[id] = { ...info, error };
    State.notify();
  };
  updateError(null);
  try {
    if (await Bridge.gameFocus() !== true) updateError("focus-failed");
  } catch {
    updateError("focus-failed");
  } finally {
    pending = false;
  }
}
