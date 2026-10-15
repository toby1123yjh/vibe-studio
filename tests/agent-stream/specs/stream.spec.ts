import { expect, test } from "@playwright/test";
import type { AgentEventEnvelope, RunState } from "../../../shared/types";
import {
  emptyCanonicalAgentTimeline,
  mergeCanonicalAgentTimeline,
} from "../../../packages/web-core/src/features/agent-runtime/model/canonicalAgentTimeline";

const event = (sequence: number): AgentEventEnvelope => ({
  schema_version: 1,
  payload_version: 1,
  event_id: `event-${sequence}`,
  session_id: "session",
  agent_run_id: "run-a",
  turn_id: "turn",
  run_attempt_id: "attempt",
  run_attempt_number: 1,
  sequence,
  correlation_id: "correlation",
  timestamp: "2026-09-08T00:00:00Z",
  native_refs: [],
  payload: {
    type: "message",
    data: {
      message: {
        message_id: "reply",
        role: "assistant",
        content: `text-${sequence}`,
      },
      final_output: false,
    },
  },
});
const state = (
  sequence: number,
  status: RunState["status"] = "running",
): RunState => ({
  state_schema_version: 1,
  reducer_version: 1,
  session_id: "session",
  agent_run_id: "run-a",
  turn_id: "turn",
  status,
  projection_status: "current",
  last_run_attempt_id: "attempt",
  last_run_attempt_number: 1,
  last_event_sequence: sequence,
  last_event_id: `event-${sequence}`,
  provider_session: null,
  terminal_output: null,
  last_error: null,
  unknown_event_count: 0,
  updated_at: "2026-09-08T00:00:00Z",
});
declare global {
  interface Window {
    streamFixture: {
      sockets: Array<{
        endpoint: string;
        transport?: {
          hostScope?: "explicit" | "current";
          hostId?: string | null;
        };
        sendEvent(value: unknown): void;
        close(): void;
      }>;
      observations: Array<{ count: number; status: string | null }>;
      frame(): void;
    };
  }
}

test("ordered append/state-only reuse old items; duplicate and reordered replay remain deterministic", () => {
  const first = mergeCanonicalAgentTimeline(emptyCanonicalAgentTimeline(), [
    event(1),
    event(3),
    event(3),
  ]);
  expect(first.events).toHaveLength(2);
  const append = mergeCanonicalAgentTimeline(first, [event(4)]);
  expect(append.items[0]).toBe(first.items[0]);
  const snapshot = mergeCanonicalAgentTimeline(append, [], state(4));
  expect(snapshot.events).toBe(append.events);
  expect(snapshot.items).toBe(append.items);
  const replay = mergeCanonicalAgentTimeline(snapshot, [event(2), event(1)]);
  expect(replay.events.map((item) => item.sequence)).toEqual([1, 2, 3, 4]);
  expect(replay.items[2]).toBe(first.items[1]);
  expect(first.events).toHaveLength(2);
  const ahead = mergeCanonicalAgentTimeline(replay, [], state(9), {
    run_attempt_number: 1,
    sequence: 9,
  });
  expect(mergeCanonicalAgentTimeline(ahead, [event(5)]).cursor?.sequence).toBe(
    9,
  );
});

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await expect
    .poll(() => page.evaluate(() => window.streamFixture?.sockets.length))
    .toBe(1);
});

test("a burst publishes once per frame and terminal flush contains the last text", async ({
  page,
}) => {
  const messages = Array.from({ length: 100 }, (_, i) => ({
    type: "event",
    data: { event: event(i + 1), replay: false },
  }));
  await page.evaluate((messages) => {
    window.streamFixture.observations.length = 0;
    messages.forEach((message) =>
      window.streamFixture.sockets[0].sendEvent(message),
    );
    window.streamFixture.frame();
  }, messages);
  await expect(page.getByTestId("count")).toHaveText("100");
  expect(
    await page.evaluate(() =>
      window.streamFixture.observations.filter((v) => v.count > 0),
    ),
  ).toHaveLength(1);
  await page.evaluate(
    ({ last, final }) => {
      const socket = window.streamFixture.sockets[0];
      socket.sendEvent({ type: "event", data: { event: last, replay: false } });
      socket.sendEvent({ type: "state", data: { state: final } });
    },
    { last: event(101), final: state(101, "succeeded") },
  );
  await expect(page.getByTestId("status")).toHaveText("succeeded");
  await expect(page.getByTestId("count")).toHaveText("101");
  await expect(page.getByTestId("text")).toContainText("text-101");
  expect(
    await page.evaluate(() =>
      window.streamFixture.observations
        .filter((v) => v.status === "succeeded")
        .every((v) => v.count === 101),
    ),
  ).toBe(true);
});

test("background frame pause still flushes; reconnect resumes applied cursor without duplicates", async ({
  page,
}) => {
  await page.evaluate(
    (value) =>
      window.streamFixture.sockets[0].sendEvent({
        type: "event",
        data: { event: value },
      }),
    event(1),
  );
  await expect(page.getByTestId("count")).toHaveText("1");
  await page.evaluate((value) => {
    window.streamFixture.sockets[0].sendEvent({
      type: "event",
      data: { event: value },
    });
    window.streamFixture.sockets[0].close();
  }, event(2));
  await expect
    .poll(() => page.evaluate(() => window.streamFixture.sockets.length))
    .toBe(2);
  expect(
    await page.evaluate(() => window.streamFixture.sockets[1].endpoint),
  ).toContain("after_sequence=2");
  await page.evaluate(
    ({ duplicate, next, snapshot }) => {
      const socket = window.streamFixture.sockets[1];
      [duplicate, next].forEach((event) =>
        socket.sendEvent({ type: "event", data: { event } }),
      );
      socket.sendEvent({ type: "ready", data: { state: snapshot } });
    },
    { duplicate: event(2), next: event(3), snapshot: state(3) },
  );
  await expect(page.getByTestId("count")).toHaveText("3");
  await expect(page.getByTestId("ready")).toHaveText("true");
});

test("run switch drops old pending frames and rejects stale socket events", async ({
  page,
}) => {
  await page.evaluate(
    (value) =>
      window.streamFixture.sockets[0].sendEvent({
        type: "event",
        data: { event: value },
      }),
    event(1),
  );
  await page.getByRole("button", { name: "Switch", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.streamFixture.sockets.length))
    .toBe(2);
  await page.evaluate((value) => {
    window.streamFixture.sockets[0].sendEvent({
      type: "event",
      data: { event: value },
    });
    window.streamFixture.sockets[0].close();
    window.streamFixture.frame();
  }, event(2));
  await expect(page.getByTestId("count")).toHaveText("0");
  await page.waitForTimeout(550);
  expect(await page.evaluate(() => window.streamFixture.sockets.length)).toBe(
    2,
  );
});

test("large bursts flush at the queue bound without dropping the remainder", async ({
  page,
}) => {
  const messages = Array.from({ length: 2050 }, (_, i) => event(i + 1));
  await page.evaluate((messages) => {
    messages.forEach((event) =>
      window.streamFixture.sockets[0].sendEvent({
        type: "event",
        data: { event },
      }),
    );
  }, messages);
  await expect(page.getByTestId("count")).toHaveText("2050");
});

test("changing Host for the same Run drops pending events and starts a scoped stream", async ({
  page,
}) => {
  await page.evaluate((value) => {
    window.streamFixture.sockets[0].sendEvent({
      type: "event",
      data: { event: value },
    });
    window.streamFixture.frame();
  }, event(1));
  await expect(page.getByTestId("count")).toHaveText("1");
  await page.evaluate((value) => {
    window.streamFixture.sockets[0].sendEvent({
      type: "event",
      data: { event: value },
    });
  }, event(2));

  await page.getByRole("button", { name: "Switch Host", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => window.streamFixture.sockets.length))
    .toBe(2);
  expect(
    await page.evaluate(() => window.streamFixture.sockets[1].transport),
  ).toEqual({ hostScope: "explicit", hostId: "host-b" });
  expect(
    await page.evaluate(() => window.streamFixture.sockets[1].endpoint),
  ).not.toContain("after_sequence");
  await page.evaluate((value) => {
    window.streamFixture.sockets[0].sendEvent({
      type: "event",
      data: { event: value },
    });
    window.streamFixture.frame();
  }, event(3));
  await expect(page.getByTestId("count")).toHaveText("0");

  await page.evaluate(
    (value) => {
      window.streamFixture.sockets[1].sendEvent({
        type: "event",
        data: { event: value },
      });
      window.streamFixture.frame();
    },
    {
      ...event(1),
      event_id: "host-b-event",
      payload: {
        type: "message",
        data: {
          message: {
            message_id: "host-b-reply",
            role: "assistant",
            content: "Host B reply",
          },
          final_output: true,
        },
      },
    },
  );
  await expect(page.getByTestId("count")).toHaveText("1");
  await expect(page.getByTestId("text")).toHaveText("Host B reply");
});
