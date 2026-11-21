import { bindChart, comparisonGroups, comparisonSchema } from './chartConfig';
import { widgetSchema, type Widget } from './widgets';
import { z } from 'zod/v4';

const text = z.string().trim().min(1).max(512);
const kinds = [
  'line',
  'area',
  'bar',
  'pie',
  'histogram',
  'waterfall',
  'heatmap',
  'scatter',
  'funnel',
  'box-plot',
  'sparkline',
  'ranking',
  'sankey',
  'simulation',
  'table',
  'calendar',
  'request-waterfall',
  'flame-chart',
  'stack-trace',
] as const;
const domain = z.tuple([z.number().finite(), z.number().finite()]).refine(([a, b]) => a < b);
const uncertainty = z.strictObject({
  lower: text,
  upper: text,
  style: z.enum(['bars', 'band']).default('bars'),
});
const chartSettings = z.strictObject({
  staged: z.boolean().optional(),
  startAtZero: z.boolean().optional(),
  yAxisPosition: z.enum(['left', 'right']).optional(),
  colors: z
    .record(text, z.string().regex(/^(?:#[a-fA-F0-9]{3}(?:[a-fA-F0-9]{3})?|var\(--[a-z0-9-]+\))$/))
    .optional(),
  rowDetails: z
    .array(z.array(z.tuple([text, z.union([z.string(), z.number(), z.boolean(), z.null()])])).max(16))
    .max(2000)
    .optional(),
  horizontal: z.boolean().optional(),
  sortOrder: z.enum(['original', 'ascending', 'descending']).optional(),
  xDomain: domain.optional(),
  yDomain: domain.optional(),
  xScale: z.enum(['linear', 'log']).optional(),
  yScale: z.enum(['linear', 'log']).optional(),
  xLabel: text.optional(),
  yLabel: text.optional(),
  showLegend: z.boolean().optional(),
  showValues: z.boolean().optional(),
  valueFormat: z
    .strictObject({
      style: z.enum(['decimal', 'currency', 'percent']),
      currency: z
        .string()
        .regex(/^[A-Z]{3}$/)
        .optional(),
      minimumFractionDigits: z.number().int().min(0).max(15).optional(),
      maximumFractionDigits: z.number().int().min(0).max(15).optional(),
    })
    .refine(
      (f) =>
        (f.style !== 'currency' || !!f.currency) &&
        (f.minimumFractionDigits ?? 0) <= (f.maximumFractionDigits ?? 15),
    )
    .optional(),
  innerRadius: z.number().min(0).lt(1).optional(),
  centerValue: z.union([text, z.number()]).optional(),
  centerLabel: text.optional(),
  fit: z.enum(['linear', 'quadratic']).optional(),
  beginningLabel: text.optional(),
  endingLabel: text.optional(),
  beginning: z.number().finite().optional(),
  ending: z.number().finite().optional(),
  binLayout: z.enum(['equal', 'proportional']).optional(),
  statistic: z.enum(['count', 'frequency', 'density']).optional(),
  whiskers: z.union([z.enum(['tukey', 'minMax']), domain.refine(([a, b]) => a >= 0 && b <= 1)]).optional(),
  colorDomain: domain.optional(),
  xOrder: z.array(z.union([text, z.number()])).optional(),
  yOrder: z.array(z.union([text, z.number()])).optional(),
  pageSize: z.number().int().min(1).max(100).optional(),
  uncertainty: uncertainty.optional(),
  referenceLines: z
    .array(
      z.union([
        z.strictObject({ axis: z.enum(['x', 'y']), value: z.union([text, z.number()]), label: text }),
        z.strictObject({
          axis: z.literal('x'),
          between: z.tuple([text, text]),
          fraction: z.number().min(0).max(1).optional(),
          label: text,
        }),
      ]),
    )
    .max(32)
    .optional(),
  referenceBands: z
    .array(
      z
        .strictObject({ axis: z.enum(['x', 'y']), from: z.number(), to: z.number(), label: text })
        .refine((b) => b.from < b.to),
    )
    .max(32)
    .optional(),
  annotations: z
    .array(
      z.strictObject({
        at: z.union([text, z.number()]),
        field: text,
        label: text,
        rowIndex: z.number().int().nonnegative().optional(),
      }),
    )
    .max(64)
    .optional(),
});
const visualSchema = z
  .object({
    version: z.literal(1),
    kind: z.enum(kinds),
    columns: z.array(text).min(1).max(32),
    rows: z.array(z.array(z.union([z.string().max(2048), z.number().finite(), z.null()])).max(32)).max(2000),
    filterColumn: text.optional(),
    options: chartSettings.optional(),
    stacked: z.enum(['absolute', 'percent']).optional(),
    followUp: text.optional(),
  })
  .strict();
export type VisualSpec = z.infer<typeof visualSchema>;
export type VisualTable = Pick<VisualSpec, 'rows' | 'columns'>;
export type BoundChart = Omit<ReturnType<typeof bindChart>, 'input'> & { spec: VisualSpec };
export type VisualDocument =
  | {
      kind: 'charts';
      title: string;
      charts: BoundChart[];
      filter?: { field: string; groups: string[]; configs: unknown[] };
    }
  | { kind: 'widget'; title: string; widget: Widget }
  | { kind: 'static'; html: string; hasControls: boolean }
  | { kind: 'invalid' }
  | { kind: 'visual'; title: string; spec: VisualSpec };

export function parseVisualDocument(html: string): VisualDocument {
  // Template contents stay inert, including resource-bearing elements.
  const template = document.createElement('template');
  template.innerHTML = html;
  // Native HTML controls and CSS can provide all interaction. A figure marker
  // alone does not turn such a document into a declarative widget.
  if (!template.content.querySelector('script')) {
    const hasControls = !!template.content.querySelector('details, select, input:not([type="hidden"]), textarea');
    return { kind: 'static', html, hasControls };
  }
  const nodes = template.content.querySelectorAll('figure[data-openbitfun-visualize]');
  if (nodes.length !== 1) return { kind: 'invalid' };
  const figure = nodes[0];
  const title = figure.querySelector(':scope > figcaption')?.textContent?.trim();
  const json = figure.querySelector(':scope > script[type="application/json"]')?.textContent;
  if (!title || !json) return { kind: 'invalid' };
  try {
    const input = JSON.parse(json);
    if (
      input &&
      (Object.prototype.hasOwnProperty.call(input, 'charts') ||
        Object.prototype.hasOwnProperty.call(input, 'spec'))
    ) {
      const comparison = Object.prototype.hasOwnProperty.call(input, 'charts')
        ? comparisonSchema.parse(input)
        : null;
      const configs = comparison?.charts ?? [input];
      const charts = configs.map((v) => parseBoundChart(v));
      if (new Set(charts.map((c) => c.id)).size !== charts.length) throw Error('Duplicate chart id');
      const field = comparison?.filterColumn;
      return {
        kind: 'charts',
        title,
        charts,
        filter:
          field && comparison
            ? { field, groups: comparisonGroups(comparison.charts, field), configs }
            : undefined,
      };
    }
    if (input?.version === 2) return { kind: 'widget', title, widget: widgetSchema.parse(input) };
    const spec = visualSchema.parse(input);
    if (
      !spec.rows.length ||
      new Set(spec.columns).size !== spec.columns.length ||
      spec.rows.some((row) => row.length !== spec.columns.length) ||
      (spec.filterColumn && !spec.columns.includes(spec.filterColumn))
    )
      return { kind: 'invalid' };
    const table = filterVisualTable(spec, null);
    if (!validTable(spec, table)) return { kind: 'invalid' };
    return { kind: 'visual', title, spec };
  } catch {
    return { kind: 'invalid' };
  }
}

export function parseBoundChart(value: unknown, filter?: { field: string; value: string }): BoundChart {
  const { input, ...meta } = bindChart(value, filter);
  const spec = visualSchema.parse(input);
  if (new Set(spec.columns).size !== spec.columns.length || (!meta.empty && !validTable(spec, spec)))
    throw Error('Invalid chart');
  return { ...meta, spec };
}

export function filterVisualTable(spec: VisualSpec, group: string | null): VisualTable {
  const index = spec.filterColumn ? spec.columns.indexOf(spec.filterColumn) : -1;
  if (index < 0) return spec;
  return {
    columns: spec.columns.filter((_, i) => i !== index),
    rows: spec.rows
      .filter((row) => group === null || String(row[index] ?? '') === group)
      .map((row) => row.filter((_, i) => i !== index)),
  };
}

function validTable(spec: VisualSpec, { rows, columns }: VisualTable): boolean {
  const n = columns.length;
  if (spec.stacked && !['bar', 'area'].includes(spec.kind)) return false;
  if (
    spec.filterColumn &&
    spec.rows.some((row) => typeof row[spec.columns.indexOf(spec.filterColumn!)] !== 'string')
  )
    return false;
  const opt = spec.options;
  if (
    (opt?.xScale === 'log' && opt.xDomain && opt.xDomain[0] <= 0) ||
    (opt?.yScale === 'log' && opt.yDomain && opt.yDomain[0] <= 0)
  )
    return false;
  if (opt?.horizontal && spec.kind !== 'bar') return false;
  const u = opt?.uncertainty;
  if (
    u &&
    (!['line', 'bar', 'scatter'].includes(spec.kind) ||
      n !== 4 ||
      columns[2] !== u.lower ||
      columns[3] !== u.upper ||
      !rows.every(
        (r) =>
          typeof r[1] === 'number' &&
          typeof r[2] === 'number' &&
          typeof r[3] === 'number' &&
          r[2] <= r[1] &&
          r[1] <= r[3],
      ))
  )
    return false;
  if (opt?.yScale === 'log' && rows.some((r) => r.slice(1).some((v) => v !== null && Number(v) <= 0)))
    return false;
  if (opt?.xScale === 'log' && rows.some((r) => typeof r[0] !== 'number' || r[0] <= 0)) return false;
  switch (spec.kind) {
    case 'table':
      return n > 0;
    case 'simulation':
      return n >= 2 && rows.every((row) => row[0] !== null);
    case 'stack-trace':
      return (
        (n === 3 || n === 4) &&
        rows.every(
          ([fn, path, line, source]) =>
            typeof fn === 'string' &&
            typeof path === 'string' &&
            Number.isInteger(line) &&
            Number(line) > 0 &&
            (source == null || typeof source === 'string'),
        )
      );
    case 'calendar':
      return (
        n === 3 &&
        rows.every(
          ([start, end, name]) =>
            typeof start === 'string' &&
            typeof end === 'string' &&
            typeof name === 'string' &&
            /^\d{4}-\d\d-\d\dT\d\d:\d\d$/.test(start) &&
            /^\d{4}-\d\d-\d\dT\d\d:\d\d$/.test(end) &&
            Number.isFinite(Date.parse(start)) &&
            Number.isFinite(Date.parse(end)) &&
            start < end,
        )
      );
    case 'flame-chart':
      return (
        n === 2 &&
        rows.every(
          ([stack, weight]) =>
            typeof stack === 'string' &&
            stack.split(';').every(Boolean) &&
            stack.split(';').length <= 64 &&
            typeof weight === 'number' &&
            weight > 0,
        )
      );
    case 'request-waterfall':
      return (
        n === 3 &&
        rows.every(
          ([name, start, end]) =>
            typeof name === 'string' && typeof start === 'number' && typeof end === 'number' && end >= start,
        )
      );
    case 'sankey':
      if (opt?.staged) {
        const paths = rows.map((r) => JSON.stringify(r.slice(0, -1)));
        const total = rows.reduce((sum, r) => sum + Number(r.at(-1)), 0);
        if (
          new Set(paths).size !== paths.length ||
          !Number.isSafeInteger(total) ||
          rows.some((r) => !Number.isSafeInteger(r.at(-1)))
        )
          return false;
      }
      return n === 3 && !opt?.staged
        ? validFlows(rows)
        : (n === 3 || n === 4 || n === 5) &&
            rows.every(
              (row) =>
                row.slice(0, -1).every((v) => typeof v === 'string' && v.length > 0) &&
                typeof row[n - 1] === 'number' &&
                Number(row[n - 1]) > 0,
            );
    case 'heatmap':
      return (
        n === 3 &&
        rows.every(([x, y, v]) => x !== null && y !== null && (v === null || typeof v === 'number'))
      );
    case 'scatter':
      return (
        (n === 2 || !!u || n === 3) &&
        rows.every(
          (row) =>
            typeof row[0] === 'number' &&
            typeof row[1] === 'number' &&
            (n !== 3 || typeof row[2] === 'string'),
        )
      );
    case 'histogram':
      if (n === 1) return !opt?.binLayout && !opt?.statistic && rows.every(([v]) => typeof v === 'number');
      if (
        n !== 4 ||
        rows.length > 200 ||
        !rows.every(
          ([a, b, label, count], i) =>
            typeof a === 'number' &&
            typeof b === 'number' &&
            b > a &&
            (!i || Number(rows[i - 1][1]) <= a) &&
            typeof label === 'string' &&
            typeof count === 'number' &&
            count >= 0,
        )
      )
        return false;
      if (opt?.statistic === 'density' && !rows.some((r) => Number(r[3]) > 0)) return false;
      return (
        opt?.binLayout !== 'proportional' ||
        (opt?.statistic ?? 'count') !== 'count' ||
        rows.every(
          (r) =>
            Math.abs((Number(r[1]) - Number(r[0])) / (Number(rows[0][1]) - Number(rows[0][0])) - 1) < 1e-9,
        )
      );
    case 'box-plot':
      if (n === 2) return rows.every(([label, v]) => label !== null && typeof v === 'number');
      return (
        n === 6 &&
        rows.every(
          (row) =>
            row[0] !== null &&
            row.slice(1).every((v, i) => typeof v === 'number' && (!i || Number(row[i]) <= v)),
        )
      );
    case 'funnel':
      return (
        n === 2 &&
        rows.every(([label, v]) => label !== null && (v === null || (typeof v === 'number' && v >= 0)))
      );
    case 'pie':
      return n === 2 && rows.every(([label, v]) => label !== null && typeof v === 'number' && v >= 0);
    case 'waterfall': {
      if (n !== 2 || !rows.every(([label, v]) => label !== null && typeof v === 'number')) return false;
      const sum = rows.reduce((a, r) => a + Number(r[1]), opt?.beginning ?? 0);
      return (
        opt?.ending === undefined ||
        Math.abs(opt.ending - sum) <= 1e-9 * Math.max(1, Math.abs(sum), Math.abs(opt.ending))
      );
    }
    case 'ranking':
    case 'sparkline':
      return n === 2 && rows.every(([label, v]) => label !== null && typeof v === 'number');
    default:
      return (
        n >= 2 &&
        rows.every(
          (row) =>
            row[0] !== null &&
            row
              .slice(1)
              .every((v) => v === null || (typeof v === 'number' && (spec.stacked !== 'percent' || v >= 0))),
        )
      );
  }
}

// ECharts requires an acyclic Sankey. Validate before mounting the renderer.
function validFlows(rows: VisualTable['rows']): boolean {
  const edges = new Map<string, string[]>(),
    incoming = new Map<string, number>();
  for (const [from, to, weight] of rows) {
    if (typeof from !== 'string' || typeof to !== 'string' || typeof weight !== 'number' || weight <= 0)
      return false;
    if (!edges.has(from)) edges.set(from, []);
    edges.get(from)!.push(to);
    if (!incoming.has(from)) incoming.set(from, 0);
    incoming.set(to, (incoming.get(to) ?? 0) + 1);
  }
  const ready = [...incoming.keys()].filter((key) => !incoming.get(key));
  for (const key of ready)
    for (const next of edges.get(key) ?? []) {
      const count = incoming.get(next)! - 1;
      incoming.set(next, count);
      if (!count) ready.push(next);
    }
  return ready.length === incoming.size;
}
