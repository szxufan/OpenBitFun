import {
  ActionCard,
  Button,
  Card,
  Checkbox,
  Field,
  Icon,
  IconButton,
  Input,
  iconNames,
  Radio,
  Select,
  SegmentedControl,
  Textarea,
  type IconName,
} from '@openbitfun/ui';
import { useState, type ReactNode } from 'react';
import { define } from './oiComponentHelpers';
import type { GenUIImplementations } from '../genUILibrary';

const text = (value: unknown) => value == null ? '' : String(value);

const options = (value: unknown): Array<{ value: string; label: string; description?: string }> => (
  Array.isArray(value)
    ? value.flat().filter(Boolean).map(item => typeof item === 'string'
      ? { value: item, label: item }
      : item as { value: string; label: string; description?: string })
    : []
);

const availableIcons = new Set<string>(iconNames);

const icon = (value: unknown, size: '2xs' | 'xs' | 'sm' | 'md' | 'lg' = 'sm'): ReactNode => {
  const name = text(value).trim().replace(/([a-z0-9])([A-Z])/g, '$1-$2').replace(/[\s_]+/g, '-').toLowerCase();
  return availableIcons.has(name) ? <Icon name={name as IconName} size={size} /> : null;
};

const buttonVariant = (value: unknown): 'fill' | 'glass' | 'outline' | 'primary' | 'quiet' | 'secondary' | 'text' => ({
  solid: 'fill',
  outline: 'outline',
  soft: 'secondary',
  ghost: 'quiet',
} as Record<string, 'fill' | 'glass' | 'outline' | 'primary' | 'quiet' | 'secondary' | 'text'>)[text(value)] ?? 'outline';

const buttonSize = (value: unknown): 'xs' | 'sm' | 'md' | 'lg' => ({
  xs: 'xs', sm: 'sm', md: 'md', lg: 'lg', xl: 'lg',
} as Record<string, 'xs' | 'sm' | 'md' | 'lg'>)[text(value)] ?? 'md';

const buttonTone = (value: unknown): 'danger' | 'neutral' => (
  ['danger', 'warning'].includes(text(value)) ? 'danger' : 'neutral'
);

const cardGap = (value: unknown): 'none' | 'sm' | 'md' | 'lg' => {
  const numeric = Number(value);
  if (!Number.isFinite(numeric) || numeric <= 0) return 'none';
  if (numeric <= 2) return 'sm';
  if (numeric <= 4) return 'md';
  return 'lg';
};

const cardPadding = (value: unknown): 'none' | 'sm' | 'md' => {
  const numeric = Number(value);
  if (!Number.isFinite(numeric) || numeric <= 0) return 'none';
  return numeric <= 2 ? 'sm' : 'md';
};

const withField = (label: unknown, control: ReactNode): ReactNode => (
  label ? <Field label={text(label)} orientation="vertical" controlWidth="fill">{control as JSX.Element}</Field> : control
);

/**
 * OpenBitFun's product controls are deliberately an adapter layer. OpenUI Lang
 * keeps its component names and event contract, while the mounted DOM comes
 * from @openbitfun/ui and therefore follows the product's tokens and themes.
 */
export const openbitfunComponents: GenUIImplementations['components'] = {
  Card: define(({ slots, view }) => (
    <Card
      appearance={view.props.variant === 'soft' ? 'subtle' : 'neutral'}
      gap={cardGap(view.props.gap)}
      padding={cardPadding(view.props.padding)}
      data-openbitfun-genui="card"
    >
      {slots.children}
    </Card>
  )),
  Button: define(({ view, emit, disabled, isStreaming }) => (
    <Button
      variant={buttonVariant(view.props.variant)}
      tone={buttonTone(view.props.color)}
      size={buttonSize(view.props.size)}
      leadingIcon={view.props.icon ? icon(view.props.icon, 'xs') : undefined}
      disabled={disabled || Boolean(isStreaming) || Boolean(view.props.disabled)}
      style={view.props.block ? { width: '100%' } : undefined}
      data-openbitfun-genui="button"
      onClick={() => emit('click')}
    >
      {text(view.props.label)}
    </Button>
  )),
  IconButton: define(({ view, emit, disabled, isStreaming }) => (
    <IconButton
      aria-label={text(view.props.label || view.props.icon)}
      icon={icon(view.props.icon, 'sm') ?? text(view.props.icon)}
      variant={buttonVariant(view.props.variant) === 'outline' ? 'outline' : 'quiet'}
      size={buttonSize(view.props.size) === 'xs' ? 'xs' : buttonSize(view.props.size)}
      disabled={disabled || Boolean(isStreaming)}
      data-openbitfun-genui="icon-button"
      onClick={() => emit('click')}
    />
  )),
  Pressable: define(({ view, slots, emit, disabled, isStreaming }) => (
    <ActionCard
      selected={view.props.selected === true}
      disabled={disabled || Boolean(isStreaming)}
      data-openbitfun-genui="pressable"
      onClick={() => emit('click')}
      body={slots.children}
    />
  )),
  Input: define(({ view, emit, disabled }) => {
    const numeric = view.props.type === 'number';
    const [draft, setDraft] = useState<string | null>(null);
    const shown = draft ?? (view.props.value == null ? '' : text(view.props.value));
    const control = (
      <Input
        type={numeric ? 'text' : text(view.props.type) || 'text'}
        inputMode={numeric ? 'decimal' : undefined}
        value={shown}
        aria-label={view.props.label ? text(view.props.label) : undefined}
        placeholder={view.props.placeholder ? text(view.props.placeholder) : undefined}
        disabled={disabled}
        data-openbitfun-genui="input"
        onValueChange={value => {
          if (!numeric) {
            emit('change', value);
            return;
          }
          setDraft(value);
          const parsed = Number(value.replace(/,/g, ''));
          if (!value.trim()) emit('change', null);
          else if (Number.isFinite(parsed)) emit('change', parsed);
        }}
        onBlur={() => setDraft(null)}
      />
    );
    return withField(view.props.label, control);
  }),
  TextArea: define(({ view, emit, disabled }) => {
    const control = (
      <Textarea
        label={undefined}
        value={view.props.value == null ? '' : text(view.props.value)}
        aria-label={view.props.label ? text(view.props.label) : undefined}
        placeholder={view.props.placeholder ? text(view.props.placeholder) : undefined}
        rows={typeof view.props.rows === 'number' ? view.props.rows : undefined}
        disabled={disabled}
        data-openbitfun-genui="textarea"
        onValueChange={value => emit('change', value)}
      />
    );
    return withField(view.props.label, control);
  }),
  Select: define(({ view, emit, disabled }) => {
    const controlOptions = options(view.props.options).map(option => ({ value: option.value, label: option.label }));
    const control = (
      <Select
        options={controlOptions}
        value={view.props.value == null ? undefined : text(view.props.value)}
        aria-label={view.props.label ? text(view.props.label) : undefined}
        placeholder={view.props.placeholder ? text(view.props.placeholder) : undefined}
        disabled={disabled}
        data-openbitfun-genui="select"
        onValueChange={value => emit('change', value)}
      />
    );
    return withField(view.props.label, control);
  }),
  SegmentedControl: define(({ view, emit, disabled }) => (
    <SegmentedControl
      options={options(view.props.options).map(option => ({ value: option.value, label: option.label }))}
      value={view.props.value == null ? undefined : text(view.props.value)}
      disabled={disabled}
      distribution={view.props.block ? 'fill' : 'content'}
      variant="pills"
      data-openbitfun-genui="segmented-control"
      onValueChange={value => emit('change', value)}
    />
  )),
  Checkbox: define(({ view, emit, disabled }) => (
    <Checkbox
      checked={view.props.checked === true}
      disabled={disabled}
      label={<span style={{ textDecoration: view.props.checked && view.props.lineThrough ? 'line-through' : undefined }}>{text(view.props.label)}</span>}
      data-openbitfun-genui="checkbox"
      onCheckedChange={checked => emit('change', checked)}
    />
  )),
  DatePicker: define(({ view, emit, disabled }) => {
    const control = (
      <Input
        type="date"
        value={text(view.props.value)}
        min={view.props.min ? text(view.props.min) : undefined}
        max={view.props.max ? text(view.props.max) : undefined}
        aria-label={view.props.label ? text(view.props.label) : undefined}
        disabled={disabled}
        data-openbitfun-genui="date-picker"
        onValueChange={value => emit('change', value)}
      />
    );
    return withField(view.props.label, control);
  }),
  RadioGroup: define(({ view, emit, disabled }) => {
    const name = `openbitfun-radio-${view.key}`;
    return (
      <fieldset data-openbitfun-genui="radio-group">
        {view.props.label ? <legend>{text(view.props.label)}</legend> : null}
        {options(view.props.options).map(option => (
          <Radio
            key={option.value}
            name={name}
            value={option.value}
            checked={text(view.props.value) === option.value}
            disabled={disabled}
            label={option.label}
            description={option.description}
            onCheckedChange={checked => { if (checked) emit('change', option.value); }}
          />
        ))}
      </fieldset>
    );
  }),
};

export const openbitfunComponentNames = Object.keys(openbitfunComponents);
