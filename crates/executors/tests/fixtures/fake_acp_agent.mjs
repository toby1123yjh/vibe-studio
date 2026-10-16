// Test-only, local stdio ACP peer. Trace semantic request fields, never native
// environment values, prompts, credential-bearing configuration or error data.
import fs from 'node:fs';
import readline from 'node:readline';

const [scenario, tracePath] = process.argv.slice(2);
const sessionId = 'fixture-native-session';
const baseModel = '["provider/name","base/model"]';
const selectedModel = '["provider/name","selected/model"]';
let currentModel = baseModel;
let currentEffort = 'medium';
let pendingPrompt;

function trace(value) {
  fs.appendFileSync(tracePath, `${JSON.stringify(value)}\n`);
}
function reply(id, result) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`);
}
function reject(id) {
  // Verify the harness never echoes provider-supplied error.data or message.
  process.stdout.write(`${JSON.stringify({
    jsonrpc: '2.0', id,
    error: { code: -32602, message: 'native-private-value', data: { secret: 'native-private-value' } },
  })}\n`);
}
function message(text) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'session/update', params: {
    sessionId, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text } },
  } })}\n`);
}
function options() {
  return [
    { id: 'model', name: 'Model', type: 'select', currentValue: currentModel,
      options: [baseModel, selectedModel].map(value => ({ value, name: value })) },
    { id: 'reasoning_effort', name: 'Reasoning effort', type: 'select', currentValue: currentEffort,
      options: (currentModel === selectedModel ? ['', 'high'] : ['medium', 'low']).map(value => ({ value, name: value || 'None' })) },
    { id: 'effort', name: 'Effort', type: 'select', currentValue: currentEffort,
      options: ['', 'high', 'low', 'medium'].map(value => ({ value, name: value || 'None' })) },
  ];
}
function sessionState() {
  return {
    configOptions: options(),
    models: { currentModelId: currentModel, availableModels: [baseModel, selectedModel].map(modelId => ({ modelId, name: modelId })) },
    modes: { currentModeId: 'default', availableModes: ['default', 'auto_edit', 'yolo', 'plan'].map(id => ({ id, name: id })) },
  };
}

readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.id === 'native-permission-request' && request.result) {
    trace({ method: 'permission_response', optionId: request.result.outcome?.optionId });
    message('fresh-agent-output');
    reply(pendingPrompt, { stopReason: 'end_turn' });
    pendingPrompt = undefined;
    return;
  }
  const { method, params = {}, id } = request;
  trace({ method, ...(params.sessionId ? { sessionId: params.sessionId } : {}),
    ...(method === 'session/set_config_option' ? { configId: params.configId, value: params.value } : {}),
    ...(params.mcpServers ? { forwardedTokenPresent: params.mcpServers.some(server =>
      (server.env || []).some(entry => entry.name === 'MCP_WORKFLOW_TOKEN' && entry.value === 'a'.repeat(64))) } : {}),
  });
  switch (method) {
    case 'initialize': {
      const resume = !['missing-resume', 'load'].includes(scenario);
      reply(id, { protocolVersion: 1, authMethods: [], agentCapabilities: {
        loadSession: true, sessionCapabilities: { ...(resume ? { resume: {} } : {}), close: {} },
      } });
      break;
    }
    case 'session/new':
      reply(id, { sessionId, ...sessionState() });
      break;
    case 'session/resume':
      if (scenario === 'restore-fail') reject(id);
      else reply(id, sessionState());
      break;
    case 'session/load':
      message('old-native-history');
      reply(id, sessionState());
      setTimeout(() => message('old-native-history-tail'), 30);
      break;
    case 'session/set_model':
      if (scenario === 'gemini-model-reject') reject(id);
      else { currentModel = params.modelId; reply(id, {}); }
      break;
    case 'session/set_mode':
      if (scenario === 'gemini-mode-reject') reject(id);
      else reply(id, {});
      break;
    case 'session/set_config_option':
      if (scenario === 'config-reject') { reject(id); break; }
      if (params.configId === 'model') { currentModel = params.value; currentEffort = ''; }
      else currentEffort = params.value;
      reply(id, { configOptions: options() });
      break;
    case 'session/prompt':
      if (scenario === 'permission') {
        pendingPrompt = id;
        process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: 'native-permission-request', method: 'session/request_permission', params: {
          sessionId, toolCall: { toolCallId: 'fixture-tool', title: 'Run native tool', kind: 'execute' },
          options: [
            { optionId: 'allow-native-once', name: 'Allow once', kind: 'allow_once' },
            { optionId: 'reject-native-once', name: 'Reject once', kind: 'reject_once' },
          ],
        } })}\n`);
        break;
      }
      message('fresh-agent-output');
      if (['cancel', 'cancel-close-fail'].includes(scenario)) pendingPrompt = id;
      else reply(id, { stopReason: scenario === 'cancelled-native' ? 'cancelled' : 'end_turn',
        usage: { totalTokens: 7, inputTokens: 5, outputTokens: 2 } });
      break;
    case 'session/cancel':
      if (pendingPrompt !== undefined) { reply(pendingPrompt, { stopReason: 'cancelled' }); pendingPrompt = undefined; }
      break;
    case 'session/close':
      setTimeout(() => {
        trace({ method: 'close_flushed' });
        if (scenario === 'cancel-close-fail') reject(id);
        else reply(id, {});
      }, 30);
      break;
    default:
      reject(id);
  }
});
