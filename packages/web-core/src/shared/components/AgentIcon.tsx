import type { CSSProperties } from 'react';
import { BaseCodingAgent } from 'shared/types';
import { useTheme } from '@/shared/hooks/useTheme';
import {
  AGENT_PROVIDER_BY_EXECUTOR,
  getAgentBrandAttributes,
} from '@/shared/lib/agentProviders';
import '@/shared/styles/agent-brand.css';

type AgentIconProps = {
  agent: BaseCodingAgent | null | undefined;
  className?: string;
};

export function getAgentName(
  agent: BaseCodingAgent | null | undefined
): string {
  return agent
    ? (AGENT_PROVIDER_BY_EXECUTOR[agent]?.label ?? 'Agent')
    : 'Agent';
}

export function AgentIcon({ agent, className = 'h-4 w-4' }: AgentIconProps) {
  const { effectiveTheme } = useTheme();
  const provider = agent ? AGENT_PROVIDER_BY_EXECUTOR[agent] : undefined;
  if (!provider) return null;

  // Preserve upstream multitone marks; monochrome marks use shared tokens.
  const multitone =
    agent === BaseCodingAgent.OH_MY_PI || agent === BaseCodingAgent.OPENCODE;

  // The adjacent executor selector supplies the accessible agent name.
  // Keep this decorative icon from rendering a duplicate fallback label.
  return (
    <span
      {...getAgentBrandAttributes(agent)}
      className={`vk-agent-mark ${className}`}
      aria-hidden="true"
    >
      {multitone ? (
        <img
          src={`${provider.brand.icon}-${effectiveTheme}.svg`}
          alt=""
          className="vk-agent-mark__image"
        />
      ) : (
        <span
          className="vk-agent-mark__glyph"
          style={
            {
              '--vk-agent-icon': `url("${provider.brand.icon}")`,
            } as CSSProperties
          }
        />
      )}
    </span>
  );
}
