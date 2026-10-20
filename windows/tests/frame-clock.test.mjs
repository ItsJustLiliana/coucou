import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { responsiveFrame } from "../src/core/frame-clock.ts";
import { setPerformanceSuspended } from "../src/core/performance.ts";

test("high refresh displays cannot drive a capped animation above 60 fps", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const old = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  let now = 0, raf, count = 0, cancel;
  const clock = mock.method(performance, "now", () => now);
  globalThis.requestAnimationFrame = fn => { raf = fn; return 1; };
  globalThis.cancelAnimationFrame = () => {};
  const draw = () => { count++; cancel = responsiveFrame(draw, 1000 / 60); };
  try {
    cancel = responsiveFrame(draw, 1000 / 60);
    raf(now);
    now = 7; raf(now); // A second frame on a 144 Hz display.
    assert.equal(count, 1);
    now = 17; mock.timers.tick(10);
    assert.equal(count, 2, "the deferred frame draws at the capped cadence");
  } finally {
    cancel?.();
    clock.mock.restore();
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = old;
    mock.timers.reset();
  }
});

test("game mode stops fallback frames and late rAF, then resumes once", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const old = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  let raf, count = 0;
  globalThis.requestAnimationFrame = fn => { raf = fn; return 1; };
  globalThis.cancelAnimationFrame = () => {};
  let cancel;
  try {
    cancel = responsiveFrame(() => count++);
    const late = raf;
    setPerformanceSuspended(true);
    late(16);
    mock.timers.tick(60000);
    assert.equal(count, 0, "neither fallback nor late rAF draws during gameplay");
    setPerformanceSuspended(false);
    raf(60016);
    mock.timers.tick(64);
    assert.equal(count, 1, "resume draws once without a duplicate fallback");
  } finally {
    cancel?.();
    setPerformanceSuspended(false);
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = old;
    mock.timers.reset();
  }
});

test("frames queued and cancelled during gameplay stay cancelled after alt-tab", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const old = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  let requests = 0, count = 0;
  globalThis.requestAnimationFrame = () => { requests++; return 1; };
  globalThis.cancelAnimationFrame = () => {};
  try {
    setPerformanceSuspended(true);
    const cancel = responsiveFrame(() => count++);
    mock.timers.tick(60000);
    assert.equal(requests, 0);
    cancel();
    setPerformanceSuspended(false);
    mock.timers.tick(64);
    assert.equal(requests, 0);
    assert.equal(count, 0);
  } finally {
    setPerformanceSuspended(false);
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = old;
    mock.timers.reset();
  }
});

test("frames keep flowing without mouse input when WebView2 withholds every rAF", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const originalRaf = globalThis.requestAnimationFrame, originalCancel = globalThis.cancelAnimationFrame;
  let pending = null, count = 0;
  globalThis.requestAnimationFrame = fn => { pending = fn; return 1; };
  globalThis.cancelAnimationFrame = () => {};
  let cancel;
  const draw = () => { count++; cancel = responsiveFrame(draw); };
  try {
    cancel = responsiveFrame(draw);
    for(let i=0;i<90;i++) mock.timers.tick(32);
    assert.equal(count,90);
    const late = pending;
    cancel(); late(3000); mock.timers.tick(1000);
    assert.equal(count,90,"cancelled and late callbacks cannot restart idle animation");
  } finally {
    globalThis.requestAnimationFrame=originalRaf; globalThis.cancelAnimationFrame=originalCancel;
    mock.timers.reset();
  }
});

test("a normal animation frame cancels its timeout instead of drawing twice", () => {
  mock.timers.enable({apis:["setTimeout"]});
  const old = [globalThis.requestAnimationFrame,globalThis.cancelAnimationFrame];
  let raf, count=0;
  globalThis.requestAnimationFrame=fn=>{raf=fn;return 1;}; globalThis.cancelAnimationFrame=()=>{};
  try { responsiveFrame(()=>count++); raf(16); mock.timers.tick(64); assert.equal(count,1); }
  finally { [globalThis.requestAnimationFrame,globalThis.cancelAnimationFrame]=old;mock.timers.reset(); }
});
