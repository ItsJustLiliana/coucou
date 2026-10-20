import { afterEach, beforeEach, mock, test } from "node:test";
import assert from "node:assert/strict";
import { gameSessionMinutes, gameSessionTime, nextGameMinuteDelay } from "../src/core/game.ts";
import { WaitingRequestTracker, WaitingRequests } from "../src/core/waiting-requests.ts";
import { KNOWN_AGENTS } from "../src/island/agents.ts";
import { registerHookHandlers } from "../src/island/hooks.ts";
import { State, DEFAULT_SETTINGS } from "../src/core/state.ts";
import { emit } from "./tauri.mjs";
import { renderIntegrationCard } from "../src/views/integrations.ts";
import { installFakeDom } from "./fakedom.mjs";

installFakeDom();
registerHookHandlers({ alert() {}, setView() {}, reveal() {}, dropPin() {}, stay() {} });

beforeEach(() => {
  mock.timers.enable({ apis: ["setTimeout"] });
  WaitingRequests.clear();
  State.tasks = [];
  State.pendingApproval = null;
  State.paused = false;
  State.settings = { ...DEFAULT_SETTINGS };
  State.loadIntegrationTasks();
});
afterEach(() => { mock.timers.tick(120_000); mock.timers.reset(); WaitingRequests.clear(); });

test("session time comes from the original process start and stays stable across alt-tab", () => {
  const session = { playing: true, startedAtMs: 1_000 };
  assert.equal(gameSessionMinutes(session, 60_999), 0);
  assert.equal(nextGameMinuteDelay(session, 60_999), 1);
  assert.equal(gameSessionTime(session, 61_000), "1m");
  assert.equal(gameSessionTime(session, 3_901_000), "1h 5m");
  assert.equal(gameSessionMinutes({ ...session, focused: false }, 3_901_000), 65);
  assert.equal(gameSessionMinutes(session, 500), 0, "a clock adjustment cannot show negative time");
  assert.equal(gameSessionMinutes({ ...session, playing: false }, 61_000), null);
  assert.equal(nextGameMinuteDelay({ playing: true }), null);
  for (const startedAtMs of [null, 0, NaN, Infinity, "123"]) assert.equal(gameSessionMinutes({ playing: true, startedAtMs }, 61_000), null);
});

test("parallel requests and sessions are counted separately; repeats and question notifications are deduplicated", () => {
  const tracker = new WaitingRequestTracker();
  const first = { hook_event_name: "PermissionRequest", session_id: "one", request_id: "a", tool_name: "Bash" };
  tracker.observe("agent_codex", first);
  tracker.observe("agent_codex", first);
  tracker.observe("agent_codex", { ...first, request_id: "b", session_id: "two" });
  tracker.observe("agent_custom", { ...first, request_id: "c" });
  assert.equal(tracker.count, 3);
  tracker.observe("agent_codex", { hook_event_name: "Notification", session_id: "one", notification_type: "permission_prompt" });
  assert.equal(tracker.count, 3);
  tracker.resolve("a");
  assert.equal(tracker.count, 2);
  tracker.observe("agent_codex", { hook_event_name: "SessionEnd", session_id: "two" });
  assert.equal(tracker.count, 1);
  tracker.observe("agent_custom", { hook_event_name: "PostToolUse", session_id: "one", tool_name: "Bash" });
  assert.equal(tracker.count, 0);
});

test("questions transition to identified requests and clear when answered in the terminal", () => {
  const tracker = new WaitingRequestTracker();
  const hook = { session_id: "question", tool_name: "AskUserQuestion" };
  tracker.observe("integration_claude", { ...hook, hook_event_name: "PreToolUse" });
  tracker.observe("integration_claude", { ...hook, hook_event_name: "PermissionRequest", request_id: "q" });
  assert.equal(tracker.count, 1);
  tracker.observe("integration_claude", { ...hook, hook_event_name: "PostToolUse" });
  assert.equal(tracker.count, 0);
  tracker.observe("agent_gemini", { hook_event_name: "Notification", session_id: "g", message: "Which option?" });
  assert.equal(tracker.count, 1);
  tracker.observe("agent_gemini", { hook_event_name: "UserPromptSubmit", session_id: "g" });
  assert.equal(tracker.count, 0);
});

test("a completed parallel tool cannot clear another request from the same session", () => {
  const tracker = new WaitingRequestTracker();
  const base = { hook_event_name: "PermissionRequest", session_id: "parallel", tool_name: "Bash" };
  tracker.observe("agent_codex", { ...base, request_id: "a", tool_use_id: "tool-a" });
  tracker.observe("agent_codex", { ...base, request_id: "b", tool_use_id: "tool-b" });
  assert.equal(tracker.count, 2);
  tracker.resolve("a");
  tracker.observe("agent_codex", { ...base, hook_event_name: "PostToolUse", tool_use_id: "tool-a" });
  assert.equal(tracker.count, 1);
  tracker.observe("agent_codex", { ...base, hook_event_name: "PostToolUse", tool_use_id: "tool-b" });
  assert.equal(tracker.count, 0);
});

test("paused hook handling still tracks requests without opening an approval card", () => {
  State.paused = true;
  emit("hook", { hook_event_name: "PermissionRequest", coucou_agent: "gemini", session_id: "paused", request_id: "paused-request" });
  assert.equal(WaitingRequests.count, 1);
  assert.equal(State.pendingApproval, null);
  emit("hook", { hook_event_name: "SessionEnd", coucou_agent: "gemini", session_id: "paused" });
  assert.equal(WaitingRequests.count, 0);
  State.paused = false;
});

test("all connected agents count even when the single approval card cannot accept their requests", () => {
  const agents = [undefined, ...Object.keys(KNOWN_AGENTS), "custom-agent"];
  for (const [index, agent] of agents.entries()) emit("hook", {
    hook_event_name: "PermissionRequest", coucou_agent: agent, session_id: `session-${index}`,
    request_id: `request-${index}`, tool_name: "Bash",
  });
  assert.equal(WaitingRequests.count, agents.length);
  emit("agent-request-resolved", "request-0");
  assert.equal(WaitingRequests.count, agents.length - 1);
  for (const [index, agent] of agents.entries()) emit("hook", { hook_event_name: "SessionEnd", coucou_agent: agent, session_id: `session-${index}` });
  assert.equal(WaitingRequests.count, 0);
});

test("the Game card renders session time, waiting requests, and a visibly styled Optimalisation button", () => {
  State.integrations.integration_game = { data: { playing: true, name: "Overwatch", startedAtMs: Date.now() - 42 * 60_000 }, error: null, loaded: true, configured: true };
  WaitingRequests.observe("agent_custom", { hook_event_name: "PermissionRequest", request_id: "pending", session_id: "custom" });
  const card = renderIntegrationCard({ id: "integration_game", color: "#A78BFA" }, {});
  assert.match(card.textContent, /Session time: 42m/);
  assert.match(card.textContent, /Waiting requests: 1/);
  const button = card.children.find(child => child.tagName === "BUTTON");
  assert.equal(button.textContent, "Optimalisation");
  assert.match(button.className, /btn secondary/);
  delete State.integrations.integration_game;
});
