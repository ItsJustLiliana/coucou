import { Bridge, onEvent } from "./bridge";

let suspended = false;
let pointerBlocked = false;
const pointerListeners = new Set<(on: boolean) => void>();
const listeners = new Set<(on: boolean) => void>();
let initialized = false;

export function performanceSuspended(): boolean { return suspended; }

export function setPerformanceSuspended(on: boolean) {
  if (on === suspended) return;
  suspended = on;
  for (const listener of [...listeners]) listener(on);
  for (const listener of [...pointerListeners]) listener(pointerInputBlocked());
}

export function onPerformanceChange(listener: (on: boolean) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function pointerInputBlocked(): boolean { return suspended || pointerBlocked; }

export function setPointerBlocked(on: boolean) {
  pointerBlocked = on;
  for (const listener of [...pointerListeners]) listener(pointerInputBlocked());
}

export function onPointerProtectionChange(listener: (on: boolean) => void): () => void {
  pointerListeners.add(listener);
  return () => { pointerListeners.delete(listener); };
}

export function guardPointerEvent(event: Event): boolean {
  if (!pointerInputBlocked()) return false;
  event.preventDefault();
  event.stopImmediatePropagation();
  return true;
}

export async function initializePerformance(interactiveSettings = false) {
  if (initialized) return;
  initialized = true;
  const style = document.createElement("style");
  style.textContent = '[data-coucou-game-mode] *, [data-coucou-game-mode] *::before, [data-coucou-game-mode] *::after { animation-play-state: paused !important; } [data-coucou-pointer-blocked], [data-coucou-pointer-blocked] * { pointer-events: none !important; }';
  document.head.append(style);
  const applyStyle = (on: boolean) => { document.documentElement.toggleAttribute("data-coucou-game-mode", on); };
  onPerformanceChange(applyStyle);
  applyStyle(suspended);
  const applyPointerStyle = (on: boolean) => { document.documentElement.toggleAttribute("data-coucou-pointer-blocked", on); };
  // General must remain usable so background-game optimization can be disabled.
  if (!interactiveSettings) {
    onPointerProtectionChange(applyPointerStyle);
    applyPointerStyle(pointerInputBlocked());
  }
  for (const type of ["mouseenter", "mouseover", "mousemove", "mousedown", "mouseup", "click", "dblclick", "contextmenu", "pointerdown", "pointerup", "pointermove", "pointerover", "pointerenter", "wheel", "dragenter", "dragover", "drop"]) {
    if (!interactiveSettings) window.addEventListener(type, guardPointerEvent, { capture: true, passive: false });
  }
  let pointerReceived = false;
  await onEvent<boolean>("pointer-blocked", (on) => { pointerReceived = true; setPointerBlocked(on); });
  const pointer = await Bridge.pointerBlocked();
  if (!pointerReceived && pointer !== null) setPointerBlocked(pointer);
  let received = false;
  await onEvent<boolean>("game-performance", (on) => {
    received = true;
    setPerformanceSuspended(on);
  });
  const current = await Bridge.performanceActive();
  if (!received && current !== null) setPerformanceSuspended(current);
}
