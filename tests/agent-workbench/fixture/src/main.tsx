import * as React from "react";
import { createRoot } from "react-dom/client";
import { AgentWorkbenchComposerEditor } from "../../../../packages/web-core/src/features/agent-workbench/ui/AgentWorkbenchComposerEditor";
import { AgentWorkbenchContainer } from "../../../../packages/web-core/src/features/agent-workbench/ui/AgentWorkbenchContainer";
import { useAgentWorkbenchInspectorVisibility } from "../../../../packages/web-core/src/features/agent-workbench/ui/AgentWorkbenchInspectorVisibility";
import {
  AGENT_WORKBENCH_INSPECTOR_TABS,
  DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES,
} from "../../../../packages/web-core/src/features/agent-workbench/model/inspectorPreferences";
import { useUiPreferencesStore } from "../../../../packages/web-core/src/shared/stores/useUiPreferencesStore";
import {
  acknowledgeSessionDraft,
  createSessionDraft,
  snapshotSessionDraft,
  updateSessionDraft,
  type SessionDraftState,
  type SessionDraftSubmission,
} from "../../../../packages/web-core/src/features/agent-workbench/model/sessionDraft";
import { deriveAgentWorkbenchCapabilities } from "../../../../packages/web-core/src/features/agent-workbench/model/agentWorkbenchCapabilities";
import type { RuntimeActionPolicy } from "../../../../packages/web-core/src/shared/lib/runtimeActionPolicy";
import "../../../../packages/web-core/src/i18n/config";
import "../../../ui-foundations/fixture/src/tailwind.css";
import "../../../../packages/ui/src/styles/tokens.css";
import "./style.css";

const DRAFT_STORAGE_KEY = "agent-workbench-fixture-drafts";

function capabilityPolicy(): RuntimeActionPolicy {
  const blocked = <T extends RuntimeActionPolicy[keyof RuntimeActionPolicy]>(
    decision: T,
  ) => decision;
  return {
    send_initial: { action: "send_initial", allowed: true, reason: null },
    send_follow_up: blocked({
      action: "send_follow_up",
      allowed: false,
      reason: "provider_capability_missing",
    }),
    queue_follow_up: blocked({
      action: "queue_follow_up",
      allowed: false,
      reason: "runtime_terminal",
    }),
    cancel_queue: blocked({
      action: "cancel_queue",
      allowed: false,
      reason: "queue_empty",
    }),
    stop: { action: "stop", allowed: true, reason: null },
    approve: blocked({
      action: "approve",
      allowed: false,
      reason: "approval_required",
    }),
    request_changes: blocked({
      action: "request_changes",
      allowed: false,
      reason: "approval_required",
    }),
    answer_question: blocked({
      action: "answer_question",
      allowed: false,
      reason: "question_required",
    }),
    retry: blocked({
      action: "retry",
      allowed: false,
      reason: "unknown_runtime",
    }),
    resume: blocked({
      action: "resume",
      allowed: false,
      reason: "unknown_runtime",
    }),
  };
}

function loadDrafts(): Record<string, SessionDraftState> {
  try {
    const value = window.localStorage.getItem(DRAFT_STORAGE_KEY);
    if (value) return JSON.parse(value) as Record<string, SessionDraftState>;
  } catch {
    // The fixture mirrors the production best-effort persistence boundary.
  }
  return {
    "session-1": createSessionDraft("session-1"),
    "session-2": createSessionDraft("session-2"),
  };
}

function InspectorFixture() {
  const [activeTab, setActiveTab] = React.useState<string>(
    DEFAULT_AGENT_WORKBENCH_INSPECTOR_PREFERENCES.activeTab,
  );
  const [terminalBuffer, setTerminalBuffer] = React.useState("");
  const visible = useAgentWorkbenchInspectorVisibility();
  const [opened, setOpened] = React.useState(false);
  React.useEffect(() => {
    if (visible) setOpened(true);
  }, [visible]);
  const activePanel =
    activeTab === "changes" ? (
      <div data-testid="inspector-heavy-panel" data-panel="changes">
        2 changed files
      </div>
    ) : activeTab === "terminal" ? (
      <div data-testid="inspector-heavy-panel" data-panel="terminal">
        <label>
          Terminal buffer
          <input
            aria-label="Terminal buffer"
            value={terminalBuffer}
            onChange={(event) => setTerminalBuffer(event.target.value)}
          />
        </label>
      </div>
    ) : activeTab === "preview" ? (
      <div data-testid="inspector-heavy-panel" data-panel="preview">
        Preview process connected
      </div>
    ) : (
      <div data-testid="inspector-heavy-panel" data-panel={activeTab}>
        Workspace {activeTab}
      </div>
    );
  return (
    <div className="fixture-inspector">
      <div role="tablist" aria-label="Inspector tabs">
        {AGENT_WORKBENCH_INSPECTOR_TABS.map((tab) => (
          <button
            key={tab}
            type="button"
            role="tab"
            aria-selected={activeTab === tab}
            onClick={() => setActiveTab(tab)}
          >
            {tab[0].toUpperCase() + tab.slice(1)}
          </button>
        ))}
      </div>
      {opened ? activePanel : null}
    </div>
  );
}

function AgentWorkbenchFixture() {
  const inspectorVisible = useUiPreferencesStore(
    (state) => state.isRightSidebarVisible,
  );
  const setInspectorVisible = (visible: boolean) => {
    const store = useUiPreferencesStore.getState();
    if (visible !== store.isRightSidebarVisible) store.toggleRightSidebar();
  };
  const [sessionId, setSessionId] = React.useState("session-1");
  const [drafts, setDrafts] = React.useState(loadDrafts);
  const [submission, setSubmission] =
    React.useState<SessionDraftSubmission | null>(null);
  const capabilities = React.useMemo(
    () => deriveAgentWorkbenchCapabilities(capabilityPolicy()),
    [],
  );
  const draft = drafts[sessionId] ?? createSessionDraft(sessionId);

  React.useEffect(() => {
    window.localStorage.setItem(DRAFT_STORAGE_KEY, JSON.stringify(drafts));
  }, [drafts]);

  const updateDraft = (text: string) => {
    setDrafts((current) => ({
      ...current,
      [sessionId]: updateSessionDraft(
        current[sessionId] ?? createSessionDraft(sessionId),
        text,
      ),
    }));
  };

  const conversation = (
    <div className="fixture-conversation">
      <div className="fixture-timeline" aria-label="Conversation timeline">
        <p>User: Keep the conversation primary.</p>
        <p>Assistant: Inspector state is presentation-only.</p>
      </div>
      <div className="fixture-capabilities" aria-label="Runtime actions">
        {Object.values(capabilities).map((capability) => (
          <button
            key={capability.action}
            type="button"
            disabled={!capability.decision.allowed}
            title={capability.disabledReason ?? undefined}
          >
            {capability.label}
          </button>
        ))}
      </div>
      <AgentWorkbenchComposerEditor
        focusKey={sessionId}
        placeholder={`Message ${sessionId}`}
        value={draft.text}
        onChange={updateDraft}
        onSubmit={() => setSubmission(snapshotSessionDraft(draft))}
        disabled={false}
        onPasteFiles={() => undefined}
      />
      <div className="fixture-draft-actions">
        <button
          type="button"
          onClick={() =>
            useUiPreferencesStore
              .getState()
              .setRightMainPanelMode("files", "fixture-workspace")
          }
        >
          Open workspace file
        </button>
        <button
          type="button"
          onClick={() => setSubmission(snapshotSessionDraft(draft))}
        >
          Send snapshot
        </button>
        <button
          type="button"
          disabled={!submission}
          onClick={() => {
            if (!submission) return;
            setDrafts((current) => ({
              ...current,
              [sessionId]: acknowledgeSessionDraft(
                current[sessionId] ?? createSessionDraft(sessionId),
                submission,
              ),
            }));
            setSubmission(null);
          }}
        >
          Acknowledge send
        </button>
        <button
          type="button"
          onClick={() =>
            setSessionId((current) =>
              current === "session-1" ? "session-2" : "session-1",
            )
          }
        >
          Switch session
        </button>
      </div>
      <output data-testid="session-id">{sessionId}</output>
    </div>
  );

  return (
    <AgentWorkbenchContainer
      title="Canonical task title"
      subtitle="Task VIB-42 / /workspaces/task-1 / feature/task-1"
      conversation={conversation}
      inspector={<InspectorFixture />}
      inspectorVisible={inspectorVisible}
      onInspectorVisibleChange={setInspectorVisible}
    />
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("Agent Workbench fixture root is missing");
createRoot(root).render(<AgentWorkbenchFixture />);
