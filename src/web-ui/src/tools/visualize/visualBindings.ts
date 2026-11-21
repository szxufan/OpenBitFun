import { z } from 'zod/v4';
import type { FlowTextItem, FlowToolItem, Session } from '@/flow_chat/types/flow-chat';
import { projectEffectiveToolItem } from '@/flow_chat/utils/toolInvocationIdentity';
import { readVisualWrite } from './writeSnapshot';

const bytes = (value: string) => new TextEncoder().encode(value).length;
const interval = (min: number) => z.number().int().min(min).max(86400);
const spec = z.discriminatedUnion('type', [
  z.strictObject({ type: z.literal('json'), value: z.json() }),
  z.strictObject({
    type: z.literal('image'),
    url: z
      .string()
      .max(8192)
      .refine((value) => {
        try {
          const url = new URL(value);
          return (
            url.protocol === 'https:' && !url.username && !url.password && !url.port && !/[#\\\s]/.test(value)
          );
        } catch {
          return false;
        }
      }),
  }),
  z.strictObject({
    type: z.literal('file'),
    path: z
      .string()
      .max(512)
      .regex(/^\.\/(?:[\w-][\w.-]*\/)*[\w-][\w.-]*$/),
  }),
  z.strictObject({
    type: z.literal('library'),
    libraryFileId: z.string().regex(/^libfile_[\w-]{1,128}$/),
    refreshIntervalSeconds: interval(15),
  }),
  z.strictObject({
    type: z.literal('tool'),
    tool: z.string().min(1).max(256),
    arguments: z.record(z.string(), z.json()),
    refreshIntervalSeconds: interval(60),
  }),
]);
type Binding = z.infer<typeof spec>;
export type Declarations = Record<string, Binding>;
export interface BoundValue {
  kind: 'json' | 'file' | 'image';
  value: unknown;
}
export type BoundValues = Record<string, BoundValue>;
const name = z
  .string()
  .regex(/^[A-Za-z][\w-]{0,63}$/)
  .refine((k) => !['prototype', 'constructor'].includes(k));
const header = z.record(name, spec);

export function readBindings(html: string): Declarations | null {
  if (!html.startsWith('<!-- vis-bindings')) return {};
  const end = html.indexOf('\n-->\n');
  if (
    !html.startsWith('<!-- vis-bindings\n') ||
    end < 0 ||
    end > 32768 ||
    bytes(html.slice(0, end + 5)) > 32768
  )
    return null;
  const declaration = html.slice('<!-- vis-bindings\n'.length, end);
  if (/[<>]/.test(declaration)) return null;
  try {
    // JSON.parse discards earlier duplicate keys. Check the token stream after
    // parsing so replay agrees with the upstream validator on ambiguous input.
    const parsed = JSON.parse(declaration);
    const objects: (Set<string> | null)[] = [];
    for (const token of declaration.matchAll(/"(?:\\.|[^"\\])*"\s*:|"(?:\\.|[^"\\])*"|[[\]{}]/g)) {
      const part = token[0];
      if (part === '{' || part === '[') objects.push(part === '{' ? new Set() : null);
      else if (part === '}' || part === ']') objects.pop();
      else if (part.endsWith(':')) {
        const key = JSON.parse(part.slice(0, -1));
        const keys = objects[objects.length - 1];
        if (keys?.has(key)) return null;
        keys?.add(key);
      }
    }
    const result = header.parse(parsed);
    return Object.keys(result).length <= 8 ? result : null;
  } catch {
    return null;
  }
}

function sameArguments(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (
    !left ||
    !right ||
    typeof left !== 'object' ||
    typeof right !== 'object' ||
    Array.isArray(left) !== Array.isArray(right)
  )
    return false;
  const entries = Object.entries(left);
  return (
    entries.length === Object.keys(right).length &&
    entries.every(
      ([key, value]) =>
        Object.prototype.hasOwnProperty.call(right, key) &&
        sameArguments(value, (right as Record<string, unknown>)[key]),
    )
  );
}

export function fileValue(name: string, text: string): BoundValue | undefined {
  if (bytes(text) > 256 * 1024) return;
  const mime: Record<string, string> = {
    json: 'application/json',
    csv: 'text/csv',
    tsv: 'text/tab-separated-values',
    txt: 'text/plain',
    md: 'text/markdown',
  };
  return {
    kind: 'file',
    value: { name, text, mimeType: mime[name.split('.').pop() ?? ''] ?? 'application/octet-stream' },
  };
}

export function usesRecordedData(binding: Binding) {
  return binding.type === 'file' || binding.type === 'library' || binding.type === 'tool';
}

function hasAllBindings(declarations: Declarations, values: BoundValues) {
  return Object.keys(declarations).every(name => Object.prototype.hasOwnProperty.call(values, name));
}

/** Preserve checked values. An old, partial receipt or a deferred figure can
 * still pick up earlier calls as the producing turn finishes or is hydrated. */
export function readerBindingSnapshot(declarations: Declarations, resolve: () => BoundValues, checked?: BoundValues) {
  let snapshot = checked;
  return (): BoundValues => {
    if (snapshot && hasAllBindings(declarations, snapshot)) return snapshot;
    const available = resolve();
    if (!snapshot || Object.keys(available).some(name => !Object.prototype.hasOwnProperty.call(snapshot, name))) {
      snapshot = { ...available, ...snapshot };
    }
    return snapshot;
  };
}

export interface FigureOwner {
  sessionId?: string | null;
  turnId?: string | null;
  toolCallId?: string | null;
  uri: string;
}

/** A missing producing call can be a headless or temporarily detached reader.
 * It must not borrow data from the selected chat or a later call. */
export function inspectionBindings(declarations: Declarations, session: Session | undefined, owner?: FigureOwner) {
  if (!Object.values(declarations).some(usesRecordedData)) {
    return resolveBindingSnapshots(declarations, undefined, undefined, owner?.uri ?? '');
  }
  if (!owner?.toolCallId || !session || session.sessionId !== owner.sessionId) return;
  const turn = session.dialogTurns.find(item => item.id === owner.turnId);
  const candidates = turn?.modelRounds.flatMap(round => round.items).filter((item): item is FlowToolItem =>
    item.type === 'tool' && item.toolCall.id === owner.toolCallId && !item.importedSource);
  if (candidates?.length !== 1) return;
  const [write] = candidates;
  if (!['Write', 'Edit'].includes(projectEffectiveToolItem(write).toolName)) return;
  const values = resolveBindingSnapshots(declarations, session, write, owner.uri);
  return hasAllBindings(declarations, values) ? values : undefined;
}

// Only the producing turn, subagent, and successful inputs available before the
// figure was written can supply its data. Replay never resolves an owner path on the reader.
export function resolveBindingSnapshots(
  declarations: Declarations,
  session: Session | undefined,
  write: FlowToolItem | FlowTextItem | undefined,
  uri: string,
): BoundValues {
  const values: BoundValues = Object.create(null);
  const entries = Object.entries(declarations);
  for (const [key, binding] of entries) {
    if (binding.type === 'json') values[key] = { kind: 'json', value: binding.value };
    if (binding.type === 'image') values[key] = { kind: 'image', value: binding.url };
  }
  if (!write || !entries.some(([, b]) => usesRecordedData(b))) return values;
  const turn = session?.dialogTurns.find((t) => t.modelRounds.some((r) => r.items.includes(write)));
  for (const round of turn?.modelRounds ?? []) {
    for (const raw of round.items) {
      if (raw === write) return values;
      if (
        raw.type !== 'tool' ||
        raw.subagentSessionId !== write.subagentSessionId ||
        raw.importedSource ||
        raw.status !== 'completed' ||
        !raw.toolResult?.success
      )
        continue;
      collectValues(values, entries, raw, uri);
    }
  }
  return values;
}

function collectValues(values: BoundValues, entries: [string, Binding][], raw: FlowToolItem, uri: string) {
  const item = projectEffectiveToolItem(raw);
  for (const [key, binding] of entries) {
    if (
      binding.type === 'tool' &&
      binding.tool === item.toolName &&
      sameArguments(binding.arguments, item.toolCall.input)
    ) {
      delete values[key];
      const value = item.toolResult?.result;
      const serialized = JSON.stringify(value);
      if (serialized !== undefined && bytes(serialized) <= 32768) values[key] = { kind: 'json', value };
    }
    if ((binding.type === 'file' || binding.type === 'library') && item.toolName === 'Write') {
      const path =
        binding.type === 'file'
          ? uri.slice(0, uri.lastIndexOf('/') + 1) + binding.path.slice(2)
          : 'openbitfun://current-session/artifacts/library/' + binding.libraryFileId;
      if (item.toolResult?.result?.file_path !== path) continue;
      delete values[key];
      const content = readVisualWrite(item, path);
      if (content === undefined) continue;
      const file = fileValue(path.split('/').pop()!, content);
      if (file) values[key] = file;
    }
  }
}

// A reader starts a refresh in the ordinary chat composer. Only a submitted turn
// bearing its unique reference can update this particular figure.
export function refreshedBinding(
  session: Session | undefined,
  request: string,
  name: string,
  binding: Binding,
  uri: string,
): BoundValue | undefined {
  const turn = session?.dialogTurns.find(
    (t) => typeof t.userMessage.content === 'string' && t.userMessage.content.includes(request),
  );
  if (!turn) return;
  const values: BoundValues = Object.create(null);
  for (const round of turn.modelRounds) {
    for (const item of round.items) {
      if (
        item.type === 'tool' &&
        !item.importedSource &&
        !item.subagentSessionId &&
        item.status === 'completed' &&
        item.toolResult?.success
      ) {
        collectValues(values, [[name, binding]], item, uri);
      }
    }
  }
  return values[name];
}

export function latestSessionBinding(
  session: Session | undefined,
  name: string,
  binding: Binding,
  uri: string,
  subagentId?: string,
) {
  for (const turn of [...(session?.dialogTurns ?? [])].reverse()) {
    for (const round of [...turn.modelRounds].reverse()) {
      for (const item of [...round.items].reverse()) {
        if (
          item.type !== 'tool' ||
          item.importedSource ||
          item.subagentSessionId !== subagentId ||
          item.status !== 'completed' ||
          !item.toolResult?.success
        )
          continue;
        const values: BoundValues = Object.create(null);
        collectValues(values, [[name, binding]], item, uri);
        if (values[name]) return { id: JSON.stringify([turn.id, round.id, item.id]), data: values[name] };
      }
    }
  }
}
