import { useRef, useState, type ReactNode } from "react";
import { LexicalComposer } from "../../../../packages/web-core/node_modules/@lexical/react/LexicalComposer";
import { RichTextPlugin } from "../../../../packages/web-core/node_modules/@lexical/react/LexicalRichTextPlugin";
import { ContentEditable } from "../../../../packages/web-core/node_modules/@lexical/react/LexicalContentEditable";
import { LexicalErrorBoundary } from "../../../../packages/web-core/node_modules/@lexical/react/LexicalErrorBoundary";
import { OnChangePlugin } from "../../../../packages/web-core/node_modules/@lexical/react/LexicalOnChangePlugin";
import { $getRoot } from "lexical";
import {
  SessionChatBox,
  type ExecutionStatus,
  type SessionChatBoxEditorRenderProps,
} from "../../../../packages/ui/src/components/SessionChatBox";
import { WorkspacesMain } from "../../../../packages/ui/src/components/WorkspacesMain";

function PreviewEditor({
  onChange,
  placeholder,
}: SessionChatBoxEditorRenderProps) {
  return (
    <LexicalComposer
      initialConfig={{
        namespace: "composer-layout-preview",
        onError(error) {
          throw error;
        },
      }}
    >
      <div className="relative">
        <RichTextPlugin
          contentEditable={
            <ContentEditable
              aria-label="Message"
              className="min-h-[3lh] min-w-0 whitespace-pre-wrap text-base outline-none [overflow-wrap:anywhere]"
            />
          }
          placeholder={
            <span className="pointer-events-none absolute left-0 top-0 text-low">
              {placeholder}
            </span>
          }
          ErrorBoundary={LexicalErrorBoundary}
        />
        <OnChangePlugin
          onChange={(state) =>
            state.read(() => onChange($getRoot().getTextContent()))
          }
        />
      </div>
    </LexicalComposer>
  );
}

export function SessionComposerPreview({
  executor,
  modelSelector,
}: {
  executor: string;
  modelSelector: ReactNode;
}) {
  const containerRef = useRef<HTMLElement>(null);
  const [text, setText] = useState("");
  const [width, setWidth] = useState(600);
  const [height, setHeight] = useState(420);
  const [compact, setCompact] = useState(true);
  const [mode, setMode] = useState<ExecutionStatus | "approval">("idle");
  const [lastAction, setLastAction] = useState("");
  return (
    <>
      <label>
        Width
        <input
          aria-label="Composer width"
          type="number"
          value={width}
          onChange={(event) => setWidth(Number(event.target.value))}
        />
      </label>
      <label>
        Height
        <input
          aria-label="Conversation height"
          type="number"
          value={height}
          onChange={(event) => setHeight(Number(event.target.value))}
        />
      </label>
      <label>
        <input
          aria-label="Compact conversation"
          type="checkbox"
          checked={compact}
          onChange={(event) => setCompact(event.target.checked)}
        />
        Compact conversation
      </label>
      <select
        aria-label="Runtime state"
        value={mode}
        onChange={(event) => setMode(event.target.value as typeof mode)}
      >
        {["idle", "sending", "running", "queued", "stopping", "approval"].map(
          (value) => (
            <option key={value}>{value}</option>
          ),
        )}
      </select>
      <div
        data-testid="session-composer-preview"
        className="new-design"
        style={{ display: "flex", width, maxWidth: "100%", height }}
      >
        <WorkspacesMain
          compact={compact}
          containerRef={containerRef}
          workspaceWithSession={{ id: "layout-workspace" }}
          isLoading={false}
          isAtBottom={false}
          onScrollToBottom={() => setLastAction("scroll-to-bottom")}
          conversationContent={
            <div
              className="min-h-0 flex-1 overflow-y-auto"
              data-testid="preview-transcript"
            >
              Latest agent response
            </div>
          }
          chatBoxContent={
            <SessionChatBox
              compact={compact}
              status={mode === "approval" ? "idle" : mode}
              editor={{ value: text, onChange: setText }}
              renderEditor={(props) => <PreviewEditor {...props} />}
              actions={{
                onSend: () => setLastAction("send"),
                onQueue: () => setLastAction("queue"),
                onCancelQueue: () => setLastAction("cancel-queue"),
                onStop: () => setLastAction("stop"),
                onPasteFiles: () => undefined,
              }}
              session={{
                sessions: [
                  {
                    id: "layout-session",
                    name: "Example session",
                    created_at: new Date(0),
                    executor,
                  },
                ],
                selectedSessionId: "layout-session",
                onSelectSession: () => setLastAction("select-session"),
                onNewSession: () => setLastAction("new-session"),
                onRenameSession: () => setLastAction("rename"),
              }}
              agent={executor}
              modelSelector={modelSelector}
              todos={[]}
              supportsContextUsage
              tokenUsageInfo={null}
              onScrollToPreviousMessage={() =>
                setLastAction("previous-message")
              }
              approvalMode={{
                isActive: mode === "approval",
                onApprove: () => setLastAction("approve"),
                onRequestChanges: () => setLastAction("request-changes"),
                onDeny: () => setLastAction("deny"),
                isSubmitting: false,
                isTimedOut: false,
              }}
            />
          }
        />
      </div>
      <output data-testid="composer-action">{lastAction}</output>
      <output hidden data-testid="composer-draft">
        {text}
      </output>
    </>
  );
}
