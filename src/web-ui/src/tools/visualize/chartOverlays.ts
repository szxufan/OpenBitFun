import type { CustomSeriesRenderItem, EChartsCoreOption } from 'echarts';
import type { VisualSpec, VisualTable } from './document';

export function withOverlays(
  option: EChartsCoreOption,
  spec: VisualSpec,
  table: VisualTable,
): EChartsCoreOption {
  const o = spec.options;
  if (!o || !Array.isArray(option.series) || !option.series.length) return option;
  const base = option.series[0],
    series = [...option.series];
  if (!base || typeof base !== 'object') return option;
  const horizontal = !!o.horizontal;
  const values = table.rows;
  series[0] = {
    ...base,
    ...(o.referenceLines
      ? {
          markLine: {
            symbol: 'none',
            data: o.referenceLines
              .filter((r) => 'value' in r)
              .map((r) => ({
                name: r.label,
                [r.axis + 'Axis']: r.value,
                label: { formatter: () => r.label },
              })),
          },
        }
      : {}),
    ...(o.referenceBands
      ? {
          markArea: {
            data: o.referenceBands.map((r) => [
              { name: r.label, [r.axis + 'Axis']: r.from },
              { [r.axis + 'Axis']: r.to },
            ]),
          },
        }
      : {}),
    ...(o.annotations
      ? {
          markPoint: {
            symbolSize: 14,
            label: { position: 'top', formatter: '{b}' },
            data: o.annotations.flatMap((a) => {
              const col = table.columns.indexOf(a.field),
                row = a.rowIndex === undefined ? values.find((r) => r[0] === a.at) : values[a.rowIndex];
              return col > 0 && row && typeof row[col] === 'number'
                ? [{ name: a.label, coord: horizontal ? [row[col], row[0]] : [row[0], row[col]] }]
                : [];
            }),
          },
        }
      : {}),
  };
  for (const ref of o.referenceLines ?? []) {
    if (!('between' in ref)) continue;
    const [left, right] = ref.between.map((key) => values.findIndex((r) => r[0] === key));
    if (left < 0 || right < 0) continue;
    const renderItem: CustomSeriesRenderItem = (params, api) => {
      const grid = params.coordSys;
      if (
        !('y' in grid) ||
        typeof grid.y !== 'number' ||
        !('height' in grid) ||
        typeof grid.height !== 'number'
      )
        return;
      const a = api.coord([left, 0])[0],
        b = api.coord([right, 0])[0],
        x = a + (b - a) * (ref.fraction ?? 0.5);
      return {
        type: 'group',
        children: [
          {
            type: 'line',
            shape: { x1: x, y1: grid.y, x2: x, y2: grid.y + grid.height },
            style: { stroke: String(api.visual('color')), lineDash: [4, 3] },
          },
          { type: 'text', x, y: grid.y, style: { text: ref.label, fill: String(api.visual('color')) } },
        ],
      };
    };
    series.push({
      type: 'custom',
      renderItem,
      silent: true,
      data: [[left, right]],
      encode: { x: -1, y: -1 },
    });
  }
  if (o.uncertainty) {
    const numeric =
      spec.kind === 'scatter' || (spec.kind === 'line' && values.every((r) => typeof r[0] === 'number'));
    const key = (r: (typeof values)[number]) => (numeric ? r[0] : String(r[0]));
    if (o.uncertainty.style === 'band') {
      const ordered = numeric ? [...values].sort((a, b) => Number(a[0]) - Number(b[0])) : values;
      const renderItem: CustomSeriesRenderItem = (params, api) =>
        params.dataIndex !== 0
          ? undefined
          : {
              type: 'polygon',
              shape: {
                points: [
                  ...ordered.map((r) => api.coord([key(r) as number, Number(r[2])])),
                  ...[...ordered].reverse().map((r) => api.coord([key(r) as number, Number(r[3])])),
                ],
              },
              style: { fill: String(api.visual('color')), opacity: 0.18 },
            };
      series.push({
        type: 'custom',
        silent: true,
        renderItem,
        data: ordered.map((r) => [key(r), r[2], r[3]]),
        encode: { x: 0, y: [1, 2] },
        z: 1,
      });
    } else {
      const renderItem: CustomSeriesRenderItem = (_, api) => {
        const at = api.value(0),
          low = api.value(1),
          high = api.value(2);
        const [a, b] = [low, high].map((v) => api.coord(horizontal ? [v, at] : [at, v]));
        const style = { stroke: String(api.visual('color')), lineWidth: 2 };
        const line = (x1: number, y1: number, x2: number, y2: number) => ({
          type: 'line' as const,
          shape: { x1, y1, x2, y2 },
          style,
        });
        return {
          type: 'group',
          children: [
            line(a[0], a[1], b[0], b[1]),
            ...[a, b].map(([x, y]) => (horizontal ? line(x, y - 3, x, y + 3) : line(x - 3, y, x + 3, y))),
          ],
        };
      };
      series.push({
        type: 'custom',
        silent: true,
        renderItem,
        data: values.map((r) => [key(r), r[2], r[3]]),
        encode: horizontal ? { x: [1, 2], y: 0 } : { x: 0, y: [1, 2] },
      });
    }
  }
  return { ...option, series };
}
