interface FixtureTransportScope {
  hostScope?: 'explicit' | 'current';
  hostId?: string | null;
}

export const summaryRequests: Array<{
  hostId: string | null;
  archived: boolean;
}> = [];
export const summaryResponses = new Map<string, unknown[]>();

export async function makeLocalApiRequest(
  endpoint: string,
  options: FixtureTransportScope & RequestInit
) {
  if (endpoint !== '/api/workspaces/summaries') {
    throw new Error(`Unexpected fixture request: ${endpoint}`);
  }
  if (options.hostScope !== 'explicit') {
    throw new Error('Workspace summaries must use an explicit Host');
  }
  const hostId = options.hostId ?? null;
  const { archived } = JSON.parse(String(options.body)) as {
    archived: boolean;
  };
  summaryRequests.push({ hostId, archived });
  return new Response(
    JSON.stringify({
      success: true,
      data: {
        summaries: summaryResponses.get(`${hostId}:${archived}`) ?? [],
      },
    }),
    { headers: { 'Content-Type': 'application/json' } }
  );
}

export class FixtureSocket {
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((message: { data: string }) => void) | null = null;
  constructor(
    readonly endpoint: string,
    readonly transport?: FixtureTransportScope
  ) {}
  close() {
    this.onclose?.();
  }
  sendEvent(value: unknown) {
    this.onmessage?.({ data: JSON.stringify(value) });
  }
}
export const sockets: FixtureSocket[] = [];
export async function openLocalApiWebSocket(
  endpoint: string,
  transport?: FixtureTransportScope
) {
  const socket = new FixtureSocket(endpoint, transport);
  sockets.push(socket);
  setTimeout(() => socket.onopen?.(), 0);
  return socket;
}
