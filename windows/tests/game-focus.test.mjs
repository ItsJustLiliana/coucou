import { test } from "node:test";
import assert from "node:assert/strict";
import { Bridge } from "../src/core/bridge.ts";
import { State } from "../src/core/state.ts";
import { focusGame } from "../src/core/game.ts";
import { renderIntegrationCard } from "../src/views/integrations.ts";
import { installFakeDom } from "./fakedom.mjs";

installFakeDom();

test("failed focus shows the requested message and a successful retry clears it", async () => {
  const original = Bridge.gameFocus;
  State.integrations.integration_game = { data: { playing: true, name: "Example Game" }, loaded: true, configured: true, error: null };
  const cardText = () => renderIntegrationCard({ id: "integration_game", color: "#A78BFA" }, {}).textContent;
  try {
    for (const result of [false, null]) {
      Bridge.gameFocus = async () => result;
      await focusGame();
      assert.match(cardText(), /Sorry this action failed/);
      assert.match(cardText(), /Example Game/);
    }
    Bridge.gameFocus = async () => { throw new Error("IPC unavailable"); };
    await focusGame();
    assert.match(cardText(), /Sorry this action failed/);
    Bridge.gameFocus = async () => true;
    await focusGame();
    assert.doesNotMatch(cardText(), /Sorry this action failed/);
  } finally { Bridge.gameFocus = original; delete State.integrations.integration_game; }
});

test("repeated clicks cannot launch overlapping focus requests", async () => {
  const original = Bridge.gameFocus;
  let finish, calls = 0;
  try {
    Bridge.gameFocus = () => { calls++; return new Promise(resolve => { finish = resolve; }); };
    const first = focusGame();
    await focusGame();
    assert.equal(calls, 1);
    finish(true);
    await first;
  } finally { Bridge.gameFocus = original; }
});

test("an open game offers General only while optimization is off", () => {
  const original = Bridge.openSettingsWindow;
  const previous = State.settings.gameOptimization;
  let page;
  const card = () => renderIntegrationCard({ id: "integration_game", color: "#A78BFA" }, {});
  try {
    Bridge.openSettingsWindow = value => { page = value; return Promise.resolve(null); };
    State.settings.gameOptimization = false;
    State.integrations.integration_game = { data: { playing: true, name: "Example Game" }, loaded: true, configured: true, error: null };
    const action = card().children.find(child => child.tagName === "BUTTON");
    assert.equal(action.textContent, "Optimalisation");
    assert.ok(action.className.includes("btn secondary"));
    action.fire("click");
    assert.equal(page, "general");
    assert.equal(State.settings.gameOptimization, false, "navigation must not silently change the setting");
    State.settings.gameOptimization = true;
    assert.ok(!card().children.some(child => child.tagName === "BUTTON"));
    State.settings.gameOptimization = false;
    State.integrations.integration_game.data = { playing: false, name: null };
    assert.ok(!card().children.some(child => child.tagName === "BUTTON"));
  } finally {
    Bridge.openSettingsWindow = original;
    State.settings.gameOptimization = previous;
    delete State.integrations.integration_game;
  }
});
