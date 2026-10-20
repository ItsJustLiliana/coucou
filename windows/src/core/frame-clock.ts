/** rAF can be withheld by transparent non-activating WebView2 windows.
 * Race it against a bounded fallback; exactly one callback owns each frame. */
import { onPerformanceChange, performanceSuspended } from "./performance";

const lastDelivered = new WeakMap<(now: number) => void, number>();

export function responsiveFrame(callback: (now: number) => void, minIntervalMs = 0): () => void {
  const earliest = (lastDelivered.get(callback) ?? -Infinity) + minIntervalMs;
  let done = false;
  let raf = 0, timer = 0;
  const clear = () => { cancelAnimationFrame(raf); window.clearTimeout(timer); };
  const run = (now: number) => {
    if (done || performanceSuspended()) return;
    clear();
    if (now < earliest) {
      timer = window.setTimeout(() => run(performance.now()), earliest - now);
      return;
    }
    done = true;
    lastDelivered.set(callback, now);
    unsubscribe();
    callback(now);
  };
  const schedule = () => {
    raf = requestAnimationFrame(run);
    timer = window.setTimeout(() => run(performance.now()), 32);
  };
  const unsubscribe = onPerformanceChange((on) => {
    clear();
    if (!on && !done) schedule();
  });
  if (!performanceSuspended()) schedule();
  return () => { done = true; clear(); unsubscribe(); };
}
