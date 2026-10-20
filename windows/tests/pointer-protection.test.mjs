import { test } from "node:test";
import assert from "node:assert/strict";
import {
  guardPointerEvent, pointerInputBlocked, setPointerBlocked, setPerformanceSuspended,
} from "../src/core/performance.ts";

test("hidden cursor coordinates cannot trigger hover or click; alt-tab restores input", () => {
  try {
    setPointerBlocked(true);
    for (const type of ["mouseenter", "mousemove", "mousedown", "click", "pointerdown", "drop"]) {
      let prevented = false, stopped = false;
      assert.equal(guardPointerEvent({
        type, preventDefault() { prevented = true; }, stopImmediatePropagation() { stopped = true; },
      }), true);
      assert.ok(prevented && stopped, type);
    }
    setPointerBlocked(false);
    assert.equal(pointerInputBlocked(), false);
    assert.equal(guardPointerEvent({ preventDefault() { assert.fail(); }, stopImmediatePropagation() { assert.fail(); } }), false);
  } finally { setPointerBlocked(false); }
});

test("a game remains protected even when it shows a visible cursor in its menus", () => {
  try {
    setPerformanceSuspended(true);
    setPointerBlocked(false);
    assert.equal(pointerInputBlocked(), true);
    assert.equal(guardPointerEvent({ preventDefault() {}, stopImmediatePropagation() {} }), true);
    setPerformanceSuspended(false);
    assert.equal(pointerInputBlocked(), false);
  } finally { setPerformanceSuspended(false); setPointerBlocked(false); }
});
