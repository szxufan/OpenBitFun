import { z } from 'zod/v4';

const label = z.string().trim().min(1).max(512);
const finite = z.number().finite();
const position = z.number().int().positive();
const scalar = z.union([z.string().max(4096), finite, z.boolean()]);
const clock = z.string().regex(/^(?:[01]\d|2[0-3]):[0-5]\d$/);
const endClock = z.union([clock, z.literal('24:00')]);
const day = z
  .string()
  .regex(/^\d{4}-\d\d-\d\d$/)
  .refine((value) => {
    const parsed = new Date(value + 'T00:00:00Z');
    return Number.isFinite(parsed.getTime()) && parsed.toISOString().startsWith(value + 'T');
  });
const unique = (ids: string[]) => new Set(ids).size === ids.length;
const shape = { version: z.literal(2), description: z.string().max(2048).optional() };
const calendar = z
  .strictObject({
    ...shape,
    kind: z.literal('calendar'),
    date: day,
    view: z.enum(['day', 'week', 'month']).default('day'),
    start: clock.default('09:00'),
    end: endClock.default('17:00'),
    now: clock.optional(),
    timeZone: label.optional(),
    emptyLabel: label.optional(),
    events: z
      .array(
        z
          .strictObject({
            id: z.union([label, finite]).optional(),
            title: label,
            date: day.optional(),
            start: clock,
            end: endClock,
            tone: z.enum(['blue', 'green', 'red', 'orange', 'purple', 'yellow']).optional(),
            detail: label.optional(),
            video: z.boolean().optional(),
          })
          .refine((e) => e.start < e.end),
      )
      .max(200),
  })
  .refine((c) => c.start < c.end);
const span = z.strictObject({
  id: label,
  name: label,
  startMs: finite.nonnegative(),
  durationMs: finite.nonnegative(),
  parentId: label.nullish(),
  service: label.optional(),
  status: z.enum(['ok', 'warning', 'error']).optional(),
  attributes: z
    .record(label, scalar)
    .refine((v) => Object.keys(v).length <= 100)
    .optional(),
});
const waterfall = z
  .strictObject({
    ...shape,
    kind: z.literal('request-waterfall'),
    durationMs: finite.positive().optional(),
    criticalPath: z.array(label).max(20000).optional(),
    spans: z.array(span).min(1).max(20000),
  })
  .refine((w) => {
    const byId = new Map(w.spans.map((s) => [s.id, s]));
    if (byId.size !== w.spans.length || w.criticalPath?.some((id) => !byId.has(id))) return false;
    return w.spans.every((s) => {
      if (
        !Number.isFinite(s.startMs + s.durationMs) ||
        (w.durationMs !== undefined && s.startMs + s.durationMs > w.durationMs)
      )
        return false;
      let parent = s.parentId,
        depth = 1;
      while (parent) {
        const p = byId.get(parent);
        if (!p || ++depth > 128) return false;
        parent = p.parentId;
      }
      return true;
    });
  });
const flame = z
  .strictObject({
    ...shape,
    kind: z.literal('flame-chart'),
    threadName: label.optional(),
    frames: z.array(z.strictObject({ id: label, name: label, category: label.optional() })).max(20000),
    samples: z
      .array(z.strictObject({ stack: z.array(label).max(128), durationMs: finite.positive() }))
      .min(1)
      .max(20000),
  })
  .refine((p) => {
    const ids = new Set(p.frames.map((f) => f.id));
    return (
      ids.size === p.frames.length &&
      p.samples.reduce((n, s) => n + s.stack.length, 0) <= 100000 &&
      Number.isFinite(p.samples.reduce((n, s) => n + s.durationMs, 0)) &&
      p.samples.every((s) => s.stack.every((id) => ids.has(id)))
    );
  });
const frame = z.strictObject({
  name: label,
  file: label.optional(),
  id: label.optional(),
  line: position.optional(),
  column: position.optional(),
  kind: z.enum(['app', 'runtime']).optional(),
  async: z.boolean().optional(),
  source: z.strictObject({ startLine: position, lines: z.array(z.string().max(4096)).max(500) }).optional(),
});
const error = z.strictObject({
  name: label,
  message: z.string().max(8192),
  stack: z.string().max(262144).optional(),
});
const stack = z
  .strictObject({
    ...shape,
    kind: z.literal('stack-trace'),
    error,
    frames: z.array(frame).max(2000).default([]),
    causes: z
      .array(error.extend({ frames: z.array(frame).max(2000) }))
      .max(32)
      .default([]),
  })
  .refine((s) =>
    unique(
      [s.frames, ...s.causes.map((c) => c.frames)].flatMap((frames) =>
        frames.flatMap((f) => (f.id ? [f.id] : [])),
      ),
    ),
  );
const formField = z.union([
  z.strictObject({
    type: z.enum(['select', 'radio']).optional(),
    label,
    options: z.array(label).min(1).max(128),
  }),
  z.strictObject({ type: z.literal('checkbox'), label, value: z.boolean().default(false) }),
  z.strictObject({ type: z.literal('text'), label, value: z.string().max(2048).default('') }),
  z
    .strictObject({
      type: z.enum(['number', 'range']),
      label,
      value: finite,
      min: finite,
      max: finite,
      step: finite.positive().optional(),
    })
    .refine((f) => f.min < f.max && f.value >= f.min && f.value <= f.max),
]);
const diagramNode = z.strictObject({
  title: label,
  caption: label.optional(),
  icon: z.enum(['desktop', 'runtime', 'artifacts', 'phone', 'relay', 'server', 'workspace']).optional(),
  drawing: z.array(z.strictObject({
    d: z.string().trim().min(1).max(4096),
    tone: z.enum(['face', 'side', 'inset', 'accent', 'solid']).default('inset'),
  })).min(1).max(48).optional(),
});
const diagram = z.array(diagramNode).min(2).max(5);
const variant = z.strictObject({
  label,
  nodes: diagram.optional(),
  steps: z.array(label).min(1).max(12).optional(),
  detail: label.optional(),
}).refine(v => !!(v.nodes || v.steps || v.detail));
const outline = z.array(z.strictObject({
  title: label, summary: label, path: label.optional(), highlight: z.boolean().optional(),
})).min(1).max(24);
const panel = z
  .strictObject({
    title: label,
    text: z.string().max(4096).optional(),
    nodes: diagram.optional(),
    variants: z.array(variant).min(2).max(12).optional(),
    outline: outline.optional(),
    columns: z.array(label).min(1).max(32).optional(),
    rows: z.array(z.array(scalar.nullable()).max(32)).max(2000).optional(),
    progress: z
      .strictObject({ label, value: finite.nonnegative(), max: finite.positive() })
      .refine((p) => p.value <= p.max)
      .optional(),
    fields: z.array(formField).max(16).optional(),
    followUp: label.optional(),
  })
  .refine(
    (p) =>
      Boolean(p.columns) === Boolean(p.rows) &&
      (!p.rows || p.rows.every((r) => r.length === p.columns!.length)) &&
      unique((p.fields ?? []).map((f) => f.label)),
  );
const panels = z.strictObject({ ...shape, kind: z.literal('tabs'), panels: z.array(panel).min(1).max(12) });
const layers = z.strictObject({
  ...shape,
  kind: z.literal('layers'),
  layers: z.array(z.strictObject({
    name: label,
    summary: label,
    path: label.optional(),
    modules: z.array(label).max(24).optional(),
  })).min(2).max(12),
});
export const widgetSchema = z.discriminatedUnion('kind', [calendar, waterfall, flame, stack, panels, layers]);
export type Widget = z.infer<typeof widgetSchema>;
export type WidgetOf<K extends Widget['kind']> = Extract<Widget, { kind: K }>;

export function analyzeSpans(widget: WidgetOf<'request-waterfall'>) {
  const children = new Map<string, typeof widget.spans>();
  for (const s of widget.spans)
    if (s.parentId) {
      const group = children.get(s.parentId) ?? [];
      group.push(s);
      children.set(s.parentId, group);
    }
  return widget.spans.map((s) => {
    const end = s.startMs + s.durationMs;
    let cursor = s.startMs;
    const uncovered: [number, number][] = [];
    const nested = [...(children.get(s.id) ?? [])].sort((a, b) => a.startMs - b.startMs);
    for (const c of nested) {
      const begin = Math.max(s.startMs, c.startMs),
        stop = Math.min(end, c.startMs + c.durationMs);
      if (stop <= begin) continue;
      if (begin > cursor) uncovered.push([cursor, begin]);
      cursor = Math.max(cursor, stop);
    }
    if (cursor < end) uncovered.push([cursor, end]);
    return {
      ...s,
      end,
      uncovered,
      direct: uncovered.reduce((sum, [a, b]) => sum + b - a, 0),
      hasChildren: nested.length > 0,
    };
  });
}

export function analyzeSamples(widget: WidgetOf<'flame-chart'>) {
  type Call = { id: number; frameId: string; start: number; duration: number; self: number; depth: number };
  const calls: Call[] = [],
    active: Call[] = [];
  const totals = new Map<string, { inclusive: number; self: number }>();
  let elapsed = 0;
  for (const { stack, durationMs } of widget.samples) {
    let depth = 0;
    while (depth < stack.length && active[depth]?.frameId === stack[depth]) depth++;
    active.length = depth;
    for (; depth < stack.length; depth++) {
      const call = { id: calls.length, frameId: stack[depth], start: elapsed, duration: 0, self: 0, depth };
      active.push(call);
      calls.push(call);
    }
    for (const call of active) call.duration += durationMs;
    for (const id of new Set(stack)) {
      const total = totals.get(id) ?? { inclusive: 0, self: 0 };
      total.inclusive += durationMs;
      totals.set(id, total);
    }
    const leaf = active.at(-1);
    if (leaf) {
      leaf.self += durationMs;
      totals.get(leaf.frameId)!.self += durationMs;
    }
    elapsed += durationMs;
  }
  return { calls, totals, elapsed };
}
