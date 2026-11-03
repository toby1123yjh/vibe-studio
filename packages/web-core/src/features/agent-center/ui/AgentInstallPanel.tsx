import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CaretDownIcon } from '@phosphor-icons/react';
import { Button } from '@vibe/ui/components/Button';
import { Input } from '@vibe/ui/components/Input';
import type { BaseCodingAgent } from 'shared/types';
import type { MachineClient } from '@/shared/lib/machineClient';
import {
  agentInstallRequest,
  supportsNpmRegistry,
  isValidInstallRegistry,
  type AgentInstallJob,
} from '@/shared/lib/agentInstall';

export function AgentInstallPanel({
  client,
  executor,
  onRescan,
}: {
  client: MachineClient;
  executor: BaseCodingAgent;
  onRescan: () => Promise<void>;
}) {
  const { t } = useTranslation('common');
  const id = useId();
  const [open, setOpen] = useState(false);
  const [registry, setRegistry] = useState('');
  const [job, setJob] = useState<AgentInstallJob | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const registryInvalid =
    supportsNpmRegistry(executor) && !isValidInstallRegistry(registry);
  const mounted = useRef(true);
  const locked = useRef(false);
  const rescan = useRef(onRescan);
  rescan.current = onRescan;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    if (job?.status !== 'running' || error) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await client.getAgentInstall(job.id);
        if (cancelled) return;
        setJob(next);
        if (next.status === 'running') timer = setTimeout(poll, 1500);
        else await rescan.current();
      } catch {
        if (!cancelled) setError(true);
      }
    };
    timer = setTimeout(poll, 1500);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, job?.id, job?.status, error]);

  const start = async () => {
    if (locked.current || registryInvalid) return;
    locked.current = true;
    setPending(true);
    setError(false);
    try {
      const next = await client.startAgentInstall(
        agentInstallRequest(executor, registry)
      );
      if (!mounted.current) return;
      setJob(next);
      if (next.status !== 'running') await rescan.current();
    } catch {
      if (mounted.current) setError(true);
    } finally {
      locked.current = false;
      if (mounted.current) setPending(false);
    }
  };

  const running = pending || job?.status === 'running';
  return (
    <section className="vk-agent-center__section vk-agent-center__installer">
      <Button
        type="button"
        variant="outline"
        className="vk-agent-center__install-toggle"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(!open)}
      >
        <span className="vk-agent-center__action-label">
          {t('agentCenter.install.title')}
          <CaretDownIcon aria-hidden="true" />
        </span>
      </Button>
      {open && (
        <div id={id}>
          <p>
            {t('agentCenter.install.description', {
              host: client.target.label,
            })}
          </p>
          {supportsNpmRegistry(executor) && (
            <label>
              <span>{t('agentCenter.install.registry')}</span>
              <Input
                type="url"
                value={registry}
                disabled={running}
                onChange={(event) => setRegistry(event.target.value)}
                aria-describedby={`${id}-hint${registryInvalid ? ` ${id}-error` : ''}`}
                aria-invalid={registryInvalid}
              />
              <small id={`${id}-hint`}>
                {t('agentCenter.install.registryHint')}
              </small>
              {registryInvalid && (
                <small id={`${id}-error`} role="alert">
                  {t('agentCenter.install.registryInvalid')}
                </small>
              )}
            </label>
          )}
          {job && <p role="status">{t(`agentCenter.install.${job.status}`)}</p>}
          {(error || job?.status === 'failed') && (
            <p role="alert">
              {error
                ? t('agentCenter.install.requestError')
                : job?.error || t('agentCenter.install.failed')}
            </p>
          )}
          {job?.logs && (
            <details className="vk-agent-center__install-logs">
              <summary>{t('agentCenter.install.logs')}</summary>
              <pre tabIndex={0} aria-label={t('agentCenter.install.logs')}>
                {job.logs.slice(-32768)}
              </pre>
            </details>
          )}
          <Button
            type="button"
            className="vk-agent-center__install-start"
            disabled={(running && !error) || registryInvalid}
            loading={running && !error}
            loadingLabel={t('agentCenter.install.running')}
            onClick={() => {
              if (job?.status === 'running' && error) setError(false);
              else void start();
            }}
          >
            {error || job?.status === 'failed'
              ? t('buttons.retry')
              : t('agentCenter.install.start')}
          </Button>
        </div>
      )}
    </section>
  );
}
