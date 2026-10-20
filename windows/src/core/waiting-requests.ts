export interface RequestHook {
  hook_event_name?: string;
  request_id?: string;
  session_id?: string;
  cwd?: string;
  tool_name?: string;
  tool_use_id?: string;
  notification_type?: string;
  message?: string;
}

interface WaitingRequest {
  session: string;
  requestId?: string;
  tool: string;
  toolUseId?: string;
}

/** Independent of the single approval card and of which pills are visible. */
export class WaitingRequestTracker {
  private requests = new Map<string, WaitingRequest>();
  private completed = new Map<string, WaitingRequest[]>();

  get count(): number { return this.requests.size; }
  clear(): void { this.requests.clear(); this.completed.clear(); }

  resolve(requestId: string): boolean {
    const key = `request:${requestId}`;
    const request = this.requests.get(key);
    if (!request) return false;
    this.requests.delete(key);
    const completed = this.completed.get(request.session) ?? [];
    completed.push(request);
    this.completed.set(request.session, completed);
    return true;
  }

  observe(agentId: string, hook: RequestHook): void {
    const session = JSON.stringify([agentId, hook.session_id || hook.cwd || ""]);
    const event = hook.hook_event_name;
    const tool = hook.tool_name ?? "";
    const remove = (predicate: (request: WaitingRequest) => boolean) => {
      for (const [key, request] of this.requests) if (request.session === session && predicate(request)) this.requests.delete(key);
    };
    if (["SessionStart", "SessionEnd", "UserPromptSubmit", "Stop", "StopFailure", "Interrupt"].includes(event ?? "")) {
      remove(() => true);
      this.completed.delete(session);
      return;
    }
    if (event === "PostToolUse" || event === "PostToolUseFailure") {
      const matches = (request: WaitingRequest) => hook.tool_use_id && request.toolUseId
        ? hook.tool_use_id === request.toolUseId : !request.tool || request.tool === tool;
      // The completion of an approved request must not clear a different,
      // still-waiting parallel request for the same tool.
      const completed = this.completed.get(session) ?? [];
      const finished = completed.findIndex(matches);
      if (finished >= 0) {
        completed.splice(finished, 1);
        if (!completed.length) this.completed.delete(session);
        return;
      }
      for (const [key, request] of this.requests) {
        if (request.session === session && matches(request)) { this.requests.delete(key); break; }
      }
      return;
    }
    if (event === "PreToolUse") {
      remove(request => !request.tool);
      if (tool !== "AskUserQuestion") return;
    }
    const notification = event === "Notification" && (
      hook.notification_type === "permission_prompt" || hook.notification_type === "elicitation_dialog" ||
      (hook.message?.trim().endsWith("?") && !hook.message.toLowerCase().includes("rate limit"))
    );
    if (notification) {
      // A notification can describe the approval already reported with an ID.
      if ([...this.requests.values()].some(request => request.session === session)) return;
    } else if (event !== "PermissionRequest" && !(event === "PreToolUse" && tool === "AskUserQuestion")) {
      return;
    }
    if (event === "PermissionRequest") remove(request => !request.requestId && (!request.tool || (request.tool === tool && (!hook.tool_use_id || request.toolUseId === hook.tool_use_id))));
    const key = hook.request_id ? `request:${hook.request_id}` : `${session}:${notification ? "notification" : tool}:${hook.tool_use_id ?? ""}`;
    this.requests.set(key, { session, requestId: hook.request_id, tool: notification ? "" : tool, toolUseId: hook.tool_use_id });
  }
}

export const WaitingRequests = new WaitingRequestTracker();
