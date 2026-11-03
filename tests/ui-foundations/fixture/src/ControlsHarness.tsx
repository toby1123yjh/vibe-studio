import { useState } from 'react';
import { EyeIcon } from '../../../../packages/ui/node_modules/@phosphor-icons/react';
import { Button } from '../../../../packages/ui/src/components/Button';
import { PrimaryButton } from '../../../../packages/ui/src/components/PrimaryButton';
import { IconButton } from '../../../../packages/ui/src/components/IconButton';
import { Input } from '../../../../packages/ui/src/components/Input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../../../../packages/ui/src/components/Select';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSearchInput,
  DropdownMenuTriggerButton,
} from '../../../../packages/ui/src/components/Dropdown';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '../../../../packages/ui/src/components/Popover';

export function ControlsHarness() {
  const [busy, setBusy] = useState(false);
  const [submissions, setSubmissions] = useState(0);
  const [previews, setPreviews] = useState(0);
  const [provider, setProvider] = useState('codex');
  const [modelSearch, setModelSearch] = useState('');
  const [model, setModel] = useState('');

  return (
    <section aria-label="Shared controls" style={{ maxWidth: 480 }}>
      <div className="fixture-actions">
        <Button type="button" onClick={() => setBusy((value) => !value)}>
          Toggle busy
        </Button>
        <Button type="button" data-testid="canonical-action" loading={busy}>
          Save changes
        </Button>
      </div>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          setSubmissions((value) => value + 1);
        }}
      >
        <div className="fixture-actions">
          <PrimaryButton
            value="Save changes"
            data-testid="primary-action"
            actionIcon={busy ? 'spinner' : undefined}
          />
          <IconButton
            icon={EyeIcon}
            aria-label="Preview changes"
            onClick={() => setPreviews((value) => value + 1)}
            disabled={busy}
          />
        </div>
      </form>
      <output data-testid="submit-count">{submissions}</output>
      <output data-testid="preview-count">{previews}</output>
      <label htmlFor="control-name">Profile name</label>
      <Input id="control-name" defaultValue="Personal" />
      <Select value={provider} onValueChange={setProvider}>
        <SelectTrigger aria-label="Default agent">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="codex">Codex</SelectItem>
          <SelectItem value="claude">Claude Code</SelectItem>
          <SelectItem value="gemini" disabled>
            Gemini
          </SelectItem>
        </SelectContent>
      </Select>
      <output data-testid="selected-provider">{provider}</output>
      <DropdownMenu>
        <DropdownMenuTriggerButton label="Choose model" />
        <DropdownMenuContent>
          <DropdownMenuSearchInput
            aria-label="Find model"
            value={modelSearch}
            onValueChange={setModelSearch}
            onKeyDown={(event) => event.stopPropagation()}
          />
          {['Fast model', 'Reasoning model'].map((name) => (
            <DropdownMenuItem
              key={name}
              preventFocusOnHover
              onSelect={() => setModel(name)}
            >
              {name}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <output data-testid="selected-model">{model}</output>
      <Popover>
        <PopoverTrigger asChild>
          <Button type="button">Open long menu</Button>
        </PopoverTrigger>
        <PopoverContent data-testid="long-popover">
          {Array.from({ length: 35 }, (_, index) => (
            <p key={index} style={{ paddingBlock: 8 }}>
              Configuration option {index + 1}
            </p>
          ))}
          <Button type="button">Last option</Button>
        </PopoverContent>
      </Popover>
    </section>
  );
}
