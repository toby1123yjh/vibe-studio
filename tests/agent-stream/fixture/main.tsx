import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { useAgentRunCanonicalStream } from "@/features/agent-runtime/model/useAgentRunCanonicalStream";
import { sockets } from "./transport";
import { FixtureHostProvider } from "./host";

const frames = new Map<number, FrameRequestCallback>();
let nextFrame = 0;
// Controllable rendering clock; fallback timers still use real browser time.
window.requestAnimationFrame = (callback) => {
  frames.set(++nextFrame, callback);
  return nextFrame;
};
window.cancelAnimationFrame = (id) => {
  frames.delete(id);
};
const observations: Array<{ count: number; status: string | null }> = [];
Object.assign(window, {
  streamFixture: {
    sockets,
    observations,
    frame: () => {
      const callbacks = [...frames.values()];
      frames.clear();
      callbacks.forEach((callback) => callback(performance.now()));
    },
  },
});
function App() {
  const [hostId, setHostId] = useState<string | null>(null);
  return (
    <FixtureHostProvider value={hostId}>
      <button onClick={() => setHostId("host-b")}>Switch Host</button>
      <Stream />
    </FixtureHostProvider>
  );
}

function Stream() {
  const [run, setRun] = useState("run-a");
  const stream = useAgentRunCanonicalStream(run);
  useEffect(() => {
    observations.push({
      count: stream.timeline?.events.length ?? 0,
      status: stream.timeline?.state?.status ?? null,
    });
  }, [stream.timeline]);
  return (
    <>
      <button onClick={() => setRun("run-b")}>Switch</button>
      <output data-testid="count">{stream.timeline?.events.length ?? 0}</output>
      <output data-testid="status">
        {stream.timeline?.state?.status ?? ""}
      </output>
      <output data-testid="ready">{String(stream.isInitialized)}</output>
      <output data-testid="text">
        {stream.timeline?.items.map((item) => item.content).join("|")}
      </output>
    </>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
