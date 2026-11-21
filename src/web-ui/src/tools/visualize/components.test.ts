// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { chartOptions } from './chartOptions';
import { withOverlays } from './chartOverlays';
import { boxStatistics, fitPoints, histogram, stagedFlows } from './chartMath';
import { chartInspection } from './chartInspection';
import { analyzeSamples, analyzeSpans, widgetSchema } from './widgets';
import { parseVisualDocument, type VisualSpec } from './document';

const spec = (
  kind: VisualSpec['kind'],
  columns: string[],
  rows: VisualSpec['rows'],
  options?: VisualSpec['options'],
): VisualSpec => ({ version: 1, kind, columns, rows, options });
function options(s: VisualSpec, hidden: string[] = []) {
  const doc = parseVisualDocument(
    '<figure data-openbitfun-visualize><figcaption>Test</figcaption><script type="application/json">' +
      JSON.stringify(s) +
      '</script></figure>',
  );
  expect(doc.kind).toBe('visual');
  return withOverlays(chartOptions(s, s, ['red', 'blue'], 'black', 'grey', new Set(hidden)), s, s) as Record<
    string,
    any
  >;
}
describe('chart behavior by family', () => {
  it('line sorts numeric coordinates, keeps a missing observation and locates notes and intervals', () => {
    const s = spec(
      'line',
      ['x', 'y'],
      [
        [3, 7],
        [1, 0],
        [2, null],
      ],
      {
        referenceLines: [{ axis: 'y', value: 5, label: 'target' }],
        annotations: [{ at: 3, field: 'y', label: 'peak' }],
      },
    );
    const series = options(s).series[0];
    expect(series.data.map((r: any) => r.value)).toEqual([
      [1, 0],
      [2, null],
      [3, 7],
    ]);
    expect(series.markLine.data[0].yAxis).toBe(5);
    expect(series.markPoint.data[0].coord).toEqual([3, 7]);
    expect(
      options(
        spec('line', ['x', 'y', 'lo', 'hi'], [[0, 1, 0, 2]], {
          uncertainty: { lower: 'lo', upper: 'hi', style: 'band' },
        }),
      ).series[1].type,
    ).toBe('custom');
  });
  it('area stacks additive values and preserves genuine zeroes', () => {
    const s = options({ ...spec('area', ['x', 'a', 'b'], [['Mon', 0, 9]]), stacked: 'absolute' }).series;
    expect(s.map((a: any) => [a.stack, a.areaStyle, a.data[0].value])).toEqual([
      ['values', {}, 0],
      ['values', {}, 9],
    ]);
  });
  it('horizontal normalized bars recalculate the denominator for the visible series', () => {
    const s = {
      ...spec('bar', ['x', 'a', 'b'], [['Mon', 1, 3]], { horizontal: true }),
      stacked: 'percent' as const,
    };
    expect(options(s).series[0].data[0].value).toBe(25);
    const v = options(s, ['b']);
    expect(v.series[0].data[0].value).toBe(100);
    expect(v.yAxis.type).toBe('category');
  });
  it('pie uses a donut, a center value and exact shares', () => {
    const s = spec(
      'pie',
      ['fruit', 'kg'],
      [
        ['A', 1],
        ['B', 3],
      ],
      { centerValue: 4, showLegend: true },
    );
    const o = options(s);
    expect(o.series[0].radius).toEqual(['54%', '82%']);
    expect(o.legend.show).toBe(true);
    expect(chartInspection(s, s)[1].text).toContain('75%');
  });
  it('histogram keeps repeats and endpoint counts, and unequal bin areas integrate to one', () => {
    expect(histogram([[1], [2], [2], [3]]).reduce((n, b) => n + b.count, 0)).toBe(4);
    const rows = [
      [0, 1, 'a', 2],
      [1, 3, 'b', 2],
    ];
    const bins = histogram(rows, 'density');
    expect(bins.map((b) => b.height)).toEqual([0.5, 0.25]);
    expect(bins.reduce((n, b) => n + b.height * (b.b - b.a), 0)).toBe(1);
    expect(
      options(
        spec('histogram', ['lo', 'hi', 'bin', 'count'], rows, {
          binLayout: 'proportional',
          statistic: 'density',
        }),
      ).series[0].type,
    ).toBe('custom');
  });
  it('waterfall reconciles bookends across zero without a fabricated movement', () => {
    const s = spec(
      'waterfall',
      ['stage', 'delta'],
      [
        ['pay', -8],
        ['refund', 2],
      ],
      { beginning: 5, ending: -1 },
    );
    expect(options(s).series[0].data.map((r: any) => r.slice(1, 3))).toEqual([
      [0, 5],
      [5, -3],
      [-3, -1],
      [0, -1],
    ]);
  });
  it('heatmap respects ordering and retains a missing cell independently of zero', () => {
    const s = spec(
      'heatmap',
      ['x', 'y', 'n'],
      [
        ['B', 'T', null],
        ['A', 'T', 0],
      ],
      { xOrder: ['A', 'B'], colorDomain: [-2, 2] },
    );
    const o = options(s);
    expect(o.series[0].data).toEqual([
      [1, 0, null],
      [0, 0, 0],
    ]);
    expect(o.visualMap.min).toBe(-2);
  });
  it('scatter fits original coordinates independently per series', () => {
    expect(
      fitPoints(
        [
          [1, 1],
          [2, 4],
          [3, 9],
        ],
        2,
      )[16][1],
    ).toBeCloseTo(4);
    expect(
      fitPoints(
        [
          [1, 1],
          [1, 2],
        ],
        1,
      ),
    ).toEqual([]);
    expect(
      options(
        spec(
          'scatter',
          ['x', 'y', 'series'],
          [
            [1, 1, 'A'],
            [2, 4, 'A'],
            [1, 2, 'B'],
            [2, 2, 'B'],
          ],
          { fit: 'linear' },
        ),
      ).series.map((s: any) => s.type),
    ).toEqual(['scatter', 'line', 'scatter', 'line']);
  });
  it('funnel retains rising stages and represents an unknown observation separately from zero', () => {
    const s = spec(
      'funnel',
      ['stage', 'count'],
      [
        ['one', 3],
        ['two', 6],
        ['missing', null],
        ['last', 0],
      ],
    );
    expect(options(s).series[0].sort).toBe('none');
    expect(chartInspection(s, s)[1].text).toContain('200%');
    expect(chartInspection(s, s)[2].text).toContain('—');
  });
  it('box plot interpolates quartiles and retains outliers and reviewed endpoints', () => {
    const r = [1, 2, 2, 3, 100].map((v) => ['A', v]);
    expect(boxStatistics(r)[0]).toEqual({ label: 'A', values: [1, 2, 2, 3, 3], outliers: [100] });
    expect(options(spec('box-plot', ['country', 'sample'], r)).series[1].data).toEqual([[0, 100]]);
    expect(
      options(spec('box-plot', ['x', 'a', 'b', 'c', 'd', 'e'], [['A', 0, 1, 2, 3, 4]])).series[0].data[0],
    ).toEqual([0, 1, 2, 3, 4]);
  });
  it('sparkline omits axes and decorative points', () => {
    const o = options(
      spec(
        'sparkline',
        ['day', 'n'],
        [
          [1, 0],
          [2, 5],
        ],
      ),
    );
    expect(o.xAxis.show).toBe(false);
    expect(o.series[0].showSymbol).toBe(false);
  });
  it('ranking honors original, ascending and descending order', () => {
    const r = [
      ['A', 2],
      ['B', 1],
      ['C', 3],
    ];
    for (const [order, expected] of [
      ['original', ['A', 'B', 'C']],
      ['ascending', ['B', 'A', 'C']],
      ['descending', ['C', 'A', 'B']],
    ] as const)
      expect(options(spec('ranking', ['name', 'score'], r, { sortOrder: order })).yAxis.data).toEqual(
        expected,
      );
  });
  it('sankey aggregates shared links and distinguishes the same label in different stages', () => {
    const rows = [
      ['X', 'X', 'Z', 3],
      ['X', 'X', 'A', 2],
    ];
    const s = stagedFlows(rows);
    expect(s.links.map((l) => l.value)).toEqual([5, 3, 2]);
    expect(s.data.length).toBe(4);
    expect(options(spec('sankey', ['a', 'b', 'c', 'n'], rows)).series[0].links).toEqual(s.links);
  });
});
describe('specialist inputs', () => {
  it('request direct time subtracts the union of child intersections', () => {
    const spans = [
      { id: 'p', name: 'get', startMs: 0, durationMs: 120 },
      { id: 'a', name: 'sql', startMs: 10, durationMs: 80, parentId: 'p' },
      { id: 'b', name: 'cache', startMs: 20, durationMs: 15, parentId: 'p' },
    ];
    expect(analyzeSpans({ version: 2, kind: 'request-waterfall', spans })[0].direct).toBe(40);
    expect(
      widgetSchema.safeParse({
        version: 2,
        kind: 'request-waterfall',
        spans: spans.map((s) => ({ ...s, parentId: s.id })),
      }).success,
    ).toBe(false);
  });
  it('flame calls are chronological, idle breaks the stack, recursion does not double-count inclusive function time', () => {
    const { calls, totals, elapsed } = analyzeSamples({
      version: 2,
      kind: 'flame-chart',
      frames: [],
      samples: [
        { stack: ['a', 'a'], durationMs: 2 },
        { stack: [], durationMs: 1 },
        { stack: ['a'], durationMs: 3 },
      ],
    });
    expect(elapsed).toBe(6);
    expect(totals.get('a')).toEqual({ inclusive: 5, self: 5 });
    expect(calls.map((c) => [c.start, c.duration, c.self])).toEqual([
      [0, 2, 0],
      [0, 2, 2],
      [3, 3, 3],
    ]);
  });
  it('calendar accepts midnight end and rejects a nonexistent date', () => {
    const w = {
      version: 2,
      kind: 'calendar',
      date: '2024-02-29',
      events: [{ title: 'Delivery', start: '23:30', end: '24:00' }],
    };
    expect(widgetSchema.safeParse(w).success).toBe(true);
    expect(widgetSchema.safeParse({ ...w, date: '2025-02-29' }).success).toBe(false);
  });
  it('stack trace preserves raw causes and rejects duplicate frame ids', () => {
    const w = {
      version: 2,
      kind: 'stack-trace',
      error: { name: 'E', message: 'stop' },
      frames: [{ name: 'run', id: '1' }],
      causes: [{ name: 'C', message: 'cause', frames: [], stack: 'raw' }],
    };
    expect(widgetSchema.safeParse(w).success).toBe(true);
    expect(widgetSchema.safeParse({ ...w, frames: [...w.frames, ...w.frames] }).success).toBe(false);
  });
});

describe('portable chart data envelopes', () => {
  const parse = (config: unknown) =>
    parseVisualDocument(
      '<figure data-openbitfun-visualize><figcaption>Comparison</figcaption><script type="application/json">' +
        JSON.stringify(config) +
        '</script></figure>',
    );
  const input = {
    id: 'a',
    title: 'Samples',
    spec: {
      type: 'line',
      x: 'time',
      y: 'value',
      rowId: 'id',
      annotations: [{ kind: 'point', field: 'value', label: 'observed', id: 'n', rowId: 'second' }],
    },
    columns: ['time', 'value', 'id'],
    rows: [
      [1, 2, 'first'],
      [1, 9, 'second'],
    ],
  };
  it('uses row identity for distinct observations at the same coordinate', () => {
    const doc = parse(input);
    if (doc.kind !== 'charts') throw Error('Expected a chart');
    expect(options(doc.charts[0].spec).series[0].markPoint.data[0].coord).toEqual([1, 9]);
  });
  it('returns an explicit empty chart, rejects missing binding fields and duplicate chart IDs', () => {
    const doc = parse({ ...input, spec: { type: 'line', x: 'time', y: 'value' }, rows: [] });
    expect(doc.kind === 'charts' && doc.charts[0].empty).toBe(true);
    expect(parse({ ...input, spec: { type: 'line', x: 'time', y: 'missing' } }).kind).toBe('invalid');
    expect(parse({ charts: [input, input] }).kind).toBe('invalid');
  });
  it('retains zero and missing intersections when binding long-form data', () => {
    const doc = parse({
      id: 'series',
      title: 'Groups',
      spec: { type: 'horizontalStackedBar100', y: 'day', x: 'value', series: 'team' },
      rows: [
        { day: 'Mon', value: 0, team: 'red' },
        { day: 'Tue', value: 1, team: 'blue' },
      ],
    });
    expect(doc.kind === 'charts' && doc.charts[0].spec.rows).toEqual([
      ['Mon', 0, null],
      ['Tue', null, 1],
    ]);
  });
  it('rejects duplicate staged paths and mismatched ending balances', () => {
    expect(
      parse({
        id: 'flow',
        title: 'Flow',
        spec: { type: 'sankey', stages: ['a', 'b'], value: 'n' },
        columns: ['a', 'b', 'n'],
        rows: [
          ['A', 'B', 2],
          ['A', 'B', 3],
        ],
      }).kind,
    ).toBe('invalid');
    expect(
      parse({
        id: 'balance',
        title: 'Balance',
        spec: { type: 'waterfall', x: 'why', y: 'amount', beginning: 5, ending: 10 },
        rows: [{ why: 'Refund', amount: 2 }],
      }).kind,
    ).toBe('invalid');
  });
});
