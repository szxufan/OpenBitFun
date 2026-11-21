import { analyzeSamples } from './widgets';
import { boxStatistics, fitPoints, histogram, stagedFlows } from './chartMath';
import type { CustomSeriesRenderItem, EChartsCoreOption } from 'echarts';
import { i18nService } from '@/infrastructure/i18n';
import type { VisualSpec, VisualTable } from './document';

const format = (n: number) => i18nService.formatNumber(n, { maximumSignificantDigits: 5 });

function intervals(rows: [number, number, number, string][], horizontal: boolean, signedColors?: string[]) {
  const renderItem: CustomSeriesRenderItem = (_, api) => {
    const slot = Number(api.value(0)),
      start = Number(api.value(1)),
      end = Number(api.value(2));
    const [a, b] = [start, end].map((v) => api.coord(horizontal ? [v, slot] : [slot, v]));
    const size = api.size!([1, 1]) as number[];
    const thickness = Math.abs(size[horizontal ? 1 : 0]) * 0.7;
    return {
      type: 'rect',
      shape: horizontal
        ? {
            x: Math.min(a[0], b[0]),
            y: a[1] - thickness / 2,
            width: Math.max(1, Math.abs(b[0] - a[0])),
            height: thickness,
          }
        : {
            x: a[0] - thickness / 2,
            y: Math.min(a[1], b[1]),
            width: thickness,
            height: Math.max(1, Math.abs(b[1] - a[1])),
          },
      style: { fill: signedColors?.[end < start ? 1 : 0] ?? String(api.visual('color')) },
    };
  };
  return {
    type: 'custom',
    renderItem,
    data: rows,
    encode: horizontal ? { x: [1, 2], y: 0, tooltip: [3, 1, 2] } : { x: 0, y: [1, 2], tooltip: [3, 1, 2] },
  };
}

export function chartOptions(
  spec: VisualSpec,
  { rows, columns }: VisualTable,
  palette: string[],
  ink: string,
  gridColor: string,
  hidden: ReadonlySet<string> = new Set(),
): EChartsCoreOption {
  const o = spec.options ?? {};
  const display = (v: number) => (o.valueFormat ? i18nService.formatNumber(v, o.valueFormat) : format(v));
  const labels = rows.map((row) => String(row[0]));
  const axis = {
    type: 'value',
    axisLabel: { formatter: display },
    splitLine: { lineStyle: { color: gridColor } },
  };
  const category = (data: string[]) => ({ type: 'category', data, axisLabel: { hideOverlap: true } });
  const layout = {
    grid: { top: 28, bottom: 8, left: 10, right: 20, containLabel: true },
    xAxis: category(labels),
    yAxis: axis,
  };
  const base = {
    animation: false,
    color: palette,
    textStyle: { color: ink },
    tooltip: { trigger: 'item', renderMode: 'richText', confine: true },
    aria: { enabled: true },
  };
  switch (spec.kind) {
    case 'funnel': {
      const runs: { start: number; data: { name: string; value: number; rowIndex: number }[] }[] = [];
      rows.forEach(([label, v], i) => {
        if (typeof v !== 'number') return;
        if (!i || rows[i - 1][1] === null) runs.push({ start: i, data: [] });
        runs[runs.length - 1].data.push({ name: String(label), value: v, rowIndex: i });
      });
      const max = Math.max(...rows.map((r) => Number(r[1])));
      return {
        ...base,
        series: runs.map((r) => ({
          type: 'funnel',
          sort: 'none',
          max,
          min: 0,
          gap: 2,
          left: '12%',
          width: '76%',
          top: 4 + (92 * r.start) / rows.length + '%',
          height: (92 * r.data.length) / rows.length + '%',
          data: r.data,
          label: {
            color: ink,
            position: 'inside',
            formatter: (p: { name: string; value: number }) => p.name + ' · ' + display(p.value),
          },
        })),
      };
    }
    case 'pie':
      return {
        ...base,
        legend: { show: o.showLegend ?? false, textStyle: { color: ink } },
        title:
          o.centerValue !== undefined
            ? {
                left: 'center',
                top: 'middle',
                text: typeof o.centerValue === 'number' ? display(o.centerValue) : o.centerValue,
                subtext: o.centerLabel,
                textStyle: { color: ink },
              }
            : undefined,
        series: [
          {
            type: 'pie',
            radius: [String(82 * (o.innerRadius ?? 54 / 82)) + '%', '82%'],
            stillShowZeroSum: false,
            data: rows.map(([name, value]) => ({
              name: String(name),
              value,
              itemStyle: o.colors?.[String(name)] ? { color: o.colors[String(name)] } : undefined,
            })),
            label: {
              color: ink,
              formatter: (p: { name: string; value: number }) =>
                o.showValues ? p.name + ' · ' + display(p.value) : p.name,
            },
          },
        ],
      };
    case 'sankey':
      return {
        ...base,
        series: [
          {
            type: 'sankey',
            right: '18%',
            emphasis: { focus: 'adjacency' },
            data: [...new Set(rows.flatMap(([from, to]) => [from, to]))].map((name) => ({ name })),
            links: rows.map(([source, target, value]) => ({ source, target, value })),
            ...(columns.length > 3 || o.staged ? stagedFlows(rows) : {}),
            label: { color: ink },
          },
        ],
      };
    case 'scatter': {
      const grouped = new Map<string, VisualTable['rows']>();
      for (const r of rows) {
        const key = columns.length === 3 ? String(r[2]) : columns[1];
        const group = grouped.get(key) ?? [];
        group.push(r);
        grouped.set(key, group);
      }
      const axes = (direction: 'x' | 'y') => ({
        ...axis,
        type: o[direction === 'x' ? 'xScale' : 'yScale'] === 'log' ? 'log' : 'value',
        name: direction === 'x' ? o.xLabel : o.yLabel,
        min: o[direction === 'x' ? 'xDomain' : 'yDomain']?.[0],
        max: o[direction === 'x' ? 'xDomain' : 'yDomain']?.[1],
      });
      return {
        ...base,
        ...layout,
        xAxis: axes('x'),
        yAxis: axes('y'),
        legend: { show: grouped.size > 1, textStyle: { color: ink } },
        series: [...grouped].flatMap(([name, seriesRows]) => [
          {
            type: 'scatter',
            name,
            data: seriesRows.map((r) => ({ value: r.slice(0, 2), rowIndex: rows.indexOf(r) })),
          },
          ...(o.fit
            ? [
                {
                  type: 'line',
                  name,
                  showSymbol: false,
                  silent: true,
                  data: fitPoints(seriesRows, o.fit === 'linear' ? 1 : 2),
                },
              ]
            : []),
        ]),
      };
    }
    case 'heatmap': {
      const xs = (o.xOrder ?? [...new Set(labels)]).map(String),
        ys = (o.yOrder ?? [...new Set(rows.map((row) => String(row[1])))]).map(String);
      const values = rows.flatMap((row) => (typeof row[2] === 'number' ? [row[2]] : []));
      return {
        ...base,
        ...layout,
        xAxis: category(xs),
        yAxis: category(ys),
        visualMap: {
          show: false,
          min: o.colorDomain?.[0] ?? (values.length ? Math.min(...values) : 0),
          max: o.colorDomain?.[1] ?? (values.length ? Math.max(...values) : 1),
          inRange: { color: [gridColor, palette[0]] },
        },
        series: [
          {
            type: 'heatmap',
            label: { show: o.showValues ?? false },
            data: rows.map(([x, y, value]) => [xs.indexOf(String(x)), ys.indexOf(String(y)), value]),
          },
        ],
      };
    }
    case 'histogram': {
      const bins = histogram(rows, o.statistic);
      if (o.binLayout === 'proportional') {
        const renderItem: CustomSeriesRenderItem = (_, api) => {
          const [x, y] = api.coord([Number(api.value(0)), 0]),
            [right, top] = api.coord([Number(api.value(1)), Number(api.value(2))]);
          return {
            type: 'rect',
            shape: { x, y: top, width: Math.max(0, right - x), height: Math.max(0, y - top) },
            style: { fill: String(api.visual('color')) },
          };
        };
        return {
          ...base,
          ...layout,
          xAxis: axis,
          series: [
            {
              type: 'custom',
              renderItem,
              encode: { x: [0, 1], y: 2, tooltip: [3, 4, 2] },
              data: bins.map((b) => [b.a, b.b, b.height, b.label, b.count]),
            },
          ],
        };
      }
      return {
        ...base,
        ...layout,
        xAxis: category(bins.map((b) => b.label || format(b.a) + '–' + format(b.b))),
        series: [{ type: 'bar', data: bins.map((b) => b.height), barCategoryGap: '2%' }],
      };
    }
    case 'box-plot': {
      if (columns.length === 6)
        return { ...base, ...layout, series: [{ type: 'boxplot', data: rows.map((r) => r.slice(1)) }] };
      const groups = boxStatistics(rows, o.whiskers);
      return {
        ...base,
        ...layout,
        xAxis: category(groups.map((g) => g.label)),
        series: [
          { type: 'boxplot', data: groups.map((g) => g.values) },
          { type: 'scatter', data: groups.flatMap((g, i) => g.outliers.map((v) => [i, v])) },
        ],
      };
    }
    case 'ranking': {
      const sorted =
        o.sortOrder === 'original'
          ? rows
          : [...rows].sort((a, b) => (Number(b[1]) - Number(a[1])) * (o.sortOrder === 'ascending' ? -1 : 1));
      return {
        ...base,
        ...layout,
        xAxis: axis,
        yAxis: { ...category(sorted.map((r) => String(r[0]))), inverse: true },
        series: [
          {
            type: 'bar',
            label: {
              show: true,
              position: 'insideRight',
              formatter: (e: { value: number }) => display(e.value),
            },
            data: sorted.map((r) => r[1]),
          },
        ],
      };
    }
    case 'waterfall': {
      let total = o.beginning ?? 0;
      const bookends = o.beginning !== undefined || o.ending !== undefined;
      const startLabel = o.beginningLabel ?? columns[0],
        endLabel = o.endingLabel ?? columns[1];
      const spans: [number, number, number, string][] = rows.map(([name, delta], i) => {
        const before = total;
        total += Number(delta);
        return [i + (bookends ? 1 : 0), before, total, String(name)];
      });
      if (bookends) {
        spans.unshift([0, 0, o.beginning ?? 0, startLabel]);
        spans.push([spans.length, 0, total, endLabel]);
      }
      return {
        ...base,
        ...layout,
        xAxis: category(spans.map((r) => r[3])),
        series: [intervals(spans, false, [palette[1], palette[4]])],
      };
    }
    case 'request-waterfall':
      return {
        ...base,
        ...layout,
        xAxis: axis,
        yAxis: { ...category(labels), inverse: true },
        series: [
          intervals(
            rows.map(([name, start, end], i) => [i, Number(start), Number(end), String(name)]),
            true,
          ),
        ],
        dataZoom: [{ type: 'inside', xAxisIndex: 0 }],
      };
    case 'flame-chart': {
      const { calls } = analyzeSamples({
        version: 2,
        kind: 'flame-chart',
        frames: [],
        samples: rows.map(([stack, weight]) => ({
          stack: String(stack).split(';'),
          durationMs: Number(weight),
        })),
      });
      const depth = Math.max(...calls.map((c) => c.depth)) + 1;
      const spans: [number, number, number, string][] = calls.map((c) => [
        c.depth,
        c.start,
        c.start + c.duration,
        c.frameId,
      ]);
      return {
        ...base,
        ...layout,
        xAxis: axis,
        yAxis: category(Array.from({ length: depth }, (_, i) => format(i + 1))),
        series: [intervals(spans, true)],
        dataZoom: [{ type: 'inside', xAxisIndex: 0 }],
      };
    }
    default: {
      const spark = spec.kind === 'sparkline';
      const measures = o.uncertainty ? columns.slice(1, 2) : columns.slice(1);
      const horizontal = !!o.horizontal;
      const quantitativeX =
        !horizontal && (spec.kind === 'line' || spark) && rows.every((r) => typeof r[0] === 'number');
      const sequence = rows.map((r, i) => ({ r, i }));
      if (quantitativeX) sequence.sort((a, b) => Number(a.r[0]) - Number(b.r[0]));
      else if (o.sortOrder && o.sortOrder !== 'original')
        sequence.sort((a, b) => (Number(a.r[1]) - Number(b.r[1])) * (o.sortOrder === 'descending' ? -1 : 1));
      const categories = sequence.map(({ r }) => String(r[0]));
      const valueDomain = horizontal ? o.xDomain : o.yDomain;
      const valueAxis = {
        ...axis,
        type: o.yScale === 'log' ? 'log' : 'value',
        position: !horizontal ? o.yAxisPosition : undefined,
        scale: o.startAtZero === false,
        show: !spark,
        name: horizontal ? o.xLabel : o.yLabel,
        min: valueDomain?.[0],
        max: valueDomain?.[1],
      };
      const dimensionAxis = {
        ...(quantitativeX
          ? { ...axis, type: o.xScale === 'log' ? 'log' : 'value', min: o.xDomain?.[0], max: o.xDomain?.[1] }
          : category(categories)),
        show: !spark,
        name: horizontal ? o.yLabel : o.xLabel,
      };
      return {
        ...base,
        ...layout,
        legend: {
          show: !spark && (o.showLegend ?? measures.length > 1),
          textStyle: { color: ink },
          selected: Object.fromEntries(measures.map((m) => [m, !hidden.has(m)])),
        },
        xAxis: horizontal ? valueAxis : dimensionAxis,
        yAxis: horizontal ? { ...dimensionAxis, inverse: true } : valueAxis,
        series: measures.map((name, j) => ({
          name,
          type: spec.kind === 'bar' ? 'bar' : 'line',
          itemStyle: o.colors?.[name] ? { color: o.colors[name] } : undefined,
          ...(spec.kind === 'area' ? { areaStyle: {} } : {}),
          label: { show: o.showValues ?? false },
          stack: spec.stacked ? 'values' : undefined,
          showSymbol: !spark,
          data: sequence.map(({ r, i }) => {
            const total =
              spec.stacked === 'percent'
                ? measures.reduce((n, m, k) => n + (hidden.has(m) ? 0 : Number(r[k + 1] ?? 0)), 0)
                : 1;
            const value =
              r[j + 1] === null || !total
                ? null
                : spec.stacked === 'percent'
                  ? (Number(r[j + 1]) * 100) / total
                  : r[j + 1];
            return { rowIndex: i, value: quantitativeX ? [r[0], value] : value };
          }),
        })),
      };
    }
  }
}
