import { useEffect, useRef, useState, type ReactNode } from "react";
import { LexicalComposer } from "@lexical/react/LexicalComposer";
import { RichTextPlugin } from "@lexical/react/LexicalRichTextPlugin";
import { ContentEditable } from "@lexical/react/LexicalContentEditable";
import { LexicalErrorBoundary } from "@lexical/react/LexicalErrorBoundary";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { $convertToMarkdownString, type Transformer } from "@lexical/markdown";
import { $getRoot, type LexicalEditor } from "lexical";
import { KeyboardCommandsPlugin } from "../../../../packages/ui/src/components/KeyboardCommandsPlugin";
import { MarkdownSyncPlugin } from "../../../../packages/ui/src/components/MarkdownSyncPlugin";
import { TypeaheadOpenProvider } from "../../../../packages/ui/src/components/TypeaheadOpenContext";
import {
  SessionChatBox,
  type ExecutionStatus,
  type SessionChatBoxEditorRenderProps,
} from "../../../../packages/ui/src/components/SessionChatBox";
import { WorkspacesMain } from "../../../../packages/ui/src/components/WorkspacesMain";

const transformers: Transformer[] = [];

function CaptureEditor({
  editorRef,
}: {
  editorRef: React.MutableRefObject<LexicalEditor | null>;
}) {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    editorRef.current = editor;
    return () => {
      editorRef.current = null;
    };
  }, [editor, editorRef]);
  return null;
}

function PreviewEditor({
  value,
  onChange,
  placeholder,
  onCmdEnter,
  editorRef,
}: SessionChatBoxEditorRenderProps & {
  editorRef: React.MutableRefObject<LexicalEditor | null>;
}) {
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
        <CaptureEditor editorRef={editorRef} />
        <MarkdownSyncPlugin
          value={value}
          onChange={onChange}
          editable
          transformers={transformers}
        />
        <TypeaheadOpenProvider>
          <KeyboardCommandsPlugin
            onCmdEnter={onCmdEnter}
            transformers={transformers}
          />
        </TypeaheadOpenProvider>
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
  const [lastSubmitted, setLastSubmitted] = useState("");
  const [submitCount, setSubmitCount] = useState(0);
  const editorRef = useRef<LexicalEditor | null>(null);
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
              editor={{
                value: text,
                onChange: setText,
                getValue: () =>
                  editorRef.current?.read(() =>
                    $convertToMarkdownString(transformers),
                  ) ?? text,
              }}
              renderEditor={(props) => (
                <PreviewEditor {...props} editorRef={editorRef} />
              )}
              actions={{
                onSend: (markdown) => {
                  setLastSubmitted(markdown);
                  setSubmitCount((value) => value + 1);
                  setLastAction("send");
                  if (
                    new URLSearchParams(window.location.search).has(
                      "pendingSend",
                    )
                  ) {
                    setMode("sending");
                  }
                  if (
                    new URLSearchParams(window.location.search).has(
                      "clearOnSend",
                    )
                  ) {
                    setText("");
                    editorRef.current?.update(() => {
                      const root = $getRoot();
                      root.clear();
                      root.selectEnd();
                    });
                  }
                },
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
      <output hidden data-testid="composer-submitted">
        {lastSubmitted}
      </output>
      <output hidden data-testid="composer-submit-count">
        {submitCount}
      </output>
      <output hidden data-testid="composer-draft">
        {text}
      </output>
    </>
  );
}
