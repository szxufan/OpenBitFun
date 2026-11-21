import { z } from 'zod/v4';
import type { VisualSpec } from './document';
const field = z.string().min(1).max(512);
const scalar = z.union([z.string().max(2048), z.number().finite(), z.boolean(), z.null()]);
const chart = z.strictObject({
  id: field,
  title: field,
  description: z.string().max(4096).nullish(),
  summary: z.string().max(4096).nullish(),
  height: z.number().finite().min(32).max(1600).nullish(),
  showHeader: z.boolean().optional(),
  columns: z.array(field).min(1).max(64).optional(),
  rows: z.array(z.union([z.record(field, scalar), z.array(scalar).max(64)])).max(2000),
  spec: z.record(field, z.unknown()),
});
const families: Record<string, VisualSpec['kind']> = {
  line: 'line',
  area: 'area',
  stackedArea: 'area',
  bar: 'bar',
  horizontalBar: 'bar',
  stackedBar: 'bar',
  horizontalStackedBar: 'bar',
  stackedBar100: 'bar',
  horizontalStackedBar100: 'bar',
  sparkline: 'sparkline',
  scatter: 'scatter',
  pie: 'pie',
  histogram: 'histogram',
  waterfall: 'waterfall',
  heatmap: 'heatmap',
  leaderboard: 'ranking',
  rankedList: 'ranking',
  funnel: 'funnel',
  boxPlot: 'box-plot',
  sankey: 'sankey',
};
function requireInput(condition: unknown): asserts condition {
  if (!condition) throw Error('Invalid chart configuration');
}
type RawRow = Record<string, z.infer<typeof scalar>>;
export function bindChart(input: unknown, filter?: { field: string; value: string }) {
  const { id, title, description, summary, height, showHeader, columns, rows, spec } = chart.parse(input);
  const {
    type,
    x,
    y,
    series,
    category,
    value,
    input: form,
    stages,
    whiskers,
    valueDecimals,
    currency,
    missingValues,
    hideMissingCells,
    colorByColumn,
    annotations,
    rowId,
    tooltipFields,
    colors,
    ...rest
  } = spec;
  requireInput(typeof type === 'string' && Object.prototype.hasOwnProperty.call(families, type));
  if (columns) requireInput(new Set(columns).size === columns.length);
  let records: RawRow[] = rows.map((row) => {
    if (!columns) {
      requireInput(!Array.isArray(row));
      return row;
    }
    requireInput(Array.isArray(row) && row.length === columns.length);
    return Object.fromEntries(columns.map((key, i) => [key, row[i]]));
  });
  if (filter) records = records.filter((r) => r[filter.field] === filter.value);
  const kind = families[type],
    horizontal = type.startsWith('horizontal');
  const options: Record<string, unknown> = { ...rest };
  if (horizontal) options.horizontal = true;
  if (type === 'sankey') options.staged = true;
  if (whiskers !== undefined)
    options.whiskers =
      typeof whiskers === 'object' && whiskers !== null
        ? z
            .object({ percentiles: z.tuple([z.number(), z.number()]) })
            .parse(whiskers)
            .percentiles.map((v) => v / 100)
        : whiskers;
  if (valueDecimals !== undefined || currency !== undefined)
    options.valueFormat ??= {
      style: currency ? 'currency' : 'decimal',
      currency,
      minimumFractionDigits: valueDecimals,
      maximumFractionDigits: valueDecimals,
    };
  if (colors !== undefined) options.colors = colors;
  requireInput(missingValues === undefined || missingValues === 'gap');
  requireInput(hideMissingCells === undefined || typeof hideMissingCells === 'boolean');
  requireInput(colorByColumn === undefined || colorByColumn === false);
  const xField = horizontal ? y : x,
    measure = horizontal ? x : y;
  let fields: unknown[];
  if (['funnel', 'ranking', 'pie'].includes(kind)) fields = [category, value];
  else if (kind === 'histogram') {
    if (form === 'raw') fields = [x];
    else {
      requireInput(form === 'binned');
      const bins = z.object({ lower: field, upper: field, label: field }).parse(x);
      fields = [bins.lower, bins.upper, bins.label, y];
    }
  } else if (kind === 'heatmap') fields = [x, y, value];
  else if (kind === 'sankey') fields = [...z.array(field).min(2).max(4).parse(stages), value];
  else if (
    kind === 'box-plot' &&
    records.some((r) => Object.prototype.hasOwnProperty.call(r, 'lowerQuartile'))
  )
    fields = [x, 'minimum', 'lowerQuartile', 'median', 'upperQuartile', 'maximum'];
  else fields = [xField, ...(Array.isArray(measure) ? measure : [measure])];
  if (options.uncertainty) {
    const u = z
      .object({ lower: field, upper: field, style: z.enum(['bars', 'band']), label: field.optional() })
      .parse(options.uncertainty);
    options.uncertainty = { lower: u.lower, upper: u.upper, style: u.style };
    fields.push(u.lower, u.upper);
  }
  const names = z.array(field).parse(fields);
  const get = (r: RawRow, key: string) => {
    requireInput(Object.prototype.hasOwnProperty.call(r, key));
    return r[key];
  };
  let bound = records.map((r) => names.map((f) => get(r, f)));
  if (series !== undefined) {
    requireInput(typeof series === 'string' && names.length === 2);
    if (kind === 'scatter') {
      names.push(series);
      bound = records.map((r) => names.map((f) => get(r, f)));
    } else {
      const labels = [...new Set(records.map((r) => get(r, series)))];
      requireInput(labels.every((v) => typeof v === 'string') && labels.length <= 31);
      const byX = new Map<RawRow[string], Map<string, RawRow[string]>>();
      for (const r of records) {
        const key = get(r, names[0]),
          group = byX.get(key) ?? new Map(),
          label = get(r, series) as string;
        requireInput(!group.has(label));
        group.set(label, get(r, names[1]));
        byX.set(key, group);
      }
      names.splice(1, 1, ...(labels as string[]));
      bound = [...byX].map(([k, values]) => [k, ...labels.map((l) => values.get(l as string) ?? null)]);
    }
  }
  if (annotations !== undefined) {
    options.annotations = z
      .array(
        z.object({
          kind: z.literal('point'),
          id: field,
          field,
          label: field,
          at: z.union([field, z.number()]).optional(),
          rowId: z.union([field, z.number()]).optional(),
        }),
      )
      .parse(annotations)
      .map((a) => {
        requireInput((a.at === undefined) !== (a.rowId === undefined));
        if (a.at !== undefined) return { at: a.at, field: a.field, label: a.label };
        requireInput(typeof rowId === 'string');
        const matches = records.filter((r) => get(r, rowId) === a.rowId);
        if (!matches.length && filter) return undefined;
        requireInput(matches.length === 1);
        return {
          at: matches[0][names[0]],
          rowIndex: records.indexOf(matches[0]),
          field: a.field,
          label: a.label,
        };
      });
  }
  if (Array.isArray(options.annotations)) options.annotations = options.annotations.filter(Boolean);
  if (tooltipFields !== undefined) {
    requireInput(!series);
    const extra = z
      .array(z.strictObject({ field, label: field }))
      .max(16)
      .parse(tooltipFields);
    options.rowDetails = records.map((r) => extra.map((f) => [f.label, get(r, f.field)]));
  }
  const stacked = type.endsWith('100')
    ? 'percent'
    : type.toLowerCase().includes('stacked')
      ? 'absolute'
      : undefined;
  return {
    id,
    title,
    description,
    summary,
    height,
    showHeader,
    empty: !bound.length,
    input: { version: 1, kind, columns: names, rows: bound, options, stacked },
  };
}
export const comparisonSchema = z.strictObject({
  charts: z.array(chart).min(2).max(3),
  filterColumn: field.optional(),
});
export function comparisonGroups(configs: z.infer<typeof chart>[], filter: string) {
  const groups = new Set<string>();
  for (const c of configs) {
    const index = c.columns?.indexOf(filter);
    for (const r of c.rows) {
      const v = Array.isArray(r) ? r[index ?? -1] : r[filter];
      requireInput(typeof v === 'string');
      groups.add(v);
    }
  }
  return [...groups];
}
