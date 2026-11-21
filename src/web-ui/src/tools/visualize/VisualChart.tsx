import { useEffect, useMemo, useRef, useState } from 'react';
import { init, use as registerCharts } from 'echarts/core';
import {
  BarChart,
  LineChart,
  PieChart,
  ScatterChart,
  FunnelChart,
  BoxplotChart,
  HeatmapChart,
  SankeyChart,
  CustomChart,
} from 'echarts/charts';
import {
  TitleComponent,
  MarkLineComponent,
  MarkAreaComponent,
  MarkPointComponent,
  GridComponent,
  TooltipComponent,
  VisualMapComponent,
  LegendComponent,
  DataZoomComponent,
  AriaComponent,
} from 'echarts/components';
import { SVGRenderer } from 'echarts/renderers';
import { chartInspection } from './chartInspection';
import { useI18n } from '@/infrastructure/i18n';
import { withOverlays } from './chartOverlays';
import { chartOptions } from './chartOptions';
import type { VisualSpec, VisualTable } from './document';
import { useHostAppearance } from '@/infrastructure/appearance/hooks/useHostAppearance';

registerCharts([
  BarChart,
  LineChart,
  PieChart,
  ScatterChart,
  FunnelChart,
  BoxplotChart,
  HeatmapChart,
  SankeyChart,
  CustomChart,
  TitleComponent,
  MarkLineComponent,
  MarkAreaComponent,
  MarkPointComponent,
  GridComponent,
  TooltipComponent,
  VisualMapComponent,
  LegendComponent,
  DataZoomComponent,
  AriaComponent,
  SVGRenderer,
]);

export default function VisualChart({
  title,
  height,
  spec,
  table,
  onSelect,
  hidden,
  onLegend,
}: {
  title: string;
  height?: number | null;
  spec: VisualSpec;
  table: VisualTable;
  hidden: string[];
  onLegend: (names: string[]) => void;
  onSelect?: (index: number) => void;
}) {
  const { t } = useI18n('flow-chat');
  const startLabel = t('visualize.start'),
    endLabel = t('visualize.end');
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ReturnType<typeof init> | null>(null);
  const inspected = useMemo(() => chartInspection(spec, table, hidden), [spec, table, hidden]);
  const legend = useMemo(() => {
    if (spec.options?.showLegend === false) return [];
    if (spec.kind === 'pie' && spec.options?.showLegend) return table.rows.map((r) => String(r[0]));
    if (spec.kind === 'scatter' && table.columns.length === 3)
      return [...new Set(table.rows.map((r) => String(r[2])))];
    if (
      !spec.options?.uncertainty &&
      ['line', 'bar', 'area'].includes(spec.kind) &&
      (table.columns.length > 2 || spec.options?.showLegend)
    )
      return table.columns.slice(1);
    return [];
  }, [spec, table]);
  const [cursor, setCursor] = useState<number | null>(null);
  const inspect = cursor === null ? undefined : inspected[cursor];
  const handlers = useRef({ spec, table, onSelect, onLegend });
  handlers.current = { spec, table, onSelect, onLegend };
  const { scheme, revision } = useHostAppearance(ref);
  useEffect(() => {
    const node = ref.current!;
    const chart = init(node, scheme === 'dark' ? 'dark' : undefined, { renderer: 'svg' });
    chartRef.current = chart;
    chart.on('legendselectchanged', (event) => {
      const selected = (event as { selected: Record<string, boolean> }).selected;
      handlers.current.onLegend(Object.keys(selected).filter((k) => !selected[k]));
    });
    chart.on('click', (event) => {
      const { spec, table, onSelect } = handlers.current;
      if (event.componentType === 'series' && ['histogram', 'box-plot'].includes(spec.kind))
        setCursor(event.dataIndex);
      if (!onSelect || event.componentType !== 'series' || ['histogram', 'flame-chart'].includes(spec.kind))
        return;
      if (
        (spec.kind === 'box-plot' && table.columns.length === 2) ||
        (spec.kind === 'sankey' &&
          (table.columns.length > 3 || spec.options?.staged || event.dataType !== 'edge'))
      )
        return;
      let index = (event.data as { rowIndex?: number } | undefined)?.rowIndex ?? event.dataIndex;
      if (spec.kind === 'ranking') {
        const sorted =
          spec.options?.sortOrder === 'original'
            ? table.rows
            : [...table.rows].sort(
                (a, b) => (Number(b[1]) - Number(a[1])) * (spec.options?.sortOrder === 'ascending' ? -1 : 1),
              );
        index = table.rows.indexOf(sorted[index]);
      }
      if (
        spec.kind === 'waterfall' &&
        (spec.options?.beginning !== undefined || spec.options?.ending !== undefined)
      )
        index--;
      if (index >= 0 && index < table.rows.length) {
        setCursor(index);
        onSelect(index);
      }
    });
    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(node);
    return () => {
      observer.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [scheme]);
  useEffect(() => {
    const css = getComputedStyle(ref.current!);
    const token = (key: string) => css.getPropertyValue('--openbitfun-color-' + key).trim();
    const palette = [
      'accent-default',
      'status-success-content',
      'status-warning-content',
      'status-info-content',
      'status-danger-content',
      'content-secondary',
    ].map(token);
    const options = chartOptions(
      spec.kind === 'waterfall'
        ? { ...spec, options: { beginningLabel: startLabel, endingLabel: endLabel, ...spec.options } }
        : spec,
      table,
      palette,
      token('content-primary'),
      token('border-default'),
      new Set(hidden),
    );
    if (options.legend && !Array.isArray(options.legend))
      options.legend = {
        ...(options.legend as object),
        show: false,
        selected: Object.fromEntries(hidden.map((name) => [name, false])),
      };
    if (
      !['sankey', 'flame-chart', 'box-plot', 'waterfall', 'ranking', 'request-waterfall'].includes(spec.kind)
    ) {
      options.tooltip = {
        ...(options.tooltip as object),
        formatter: (params: { dataIndex: number; data?: { rowIndex?: number } }) =>
          inspected[params.data?.rowIndex ?? params.dataIndex]?.text ?? '',
      };
    }
    chartRef.current?.setOption(
      { ...withOverlays(options, spec, table), backgroundColor: 'transparent' },
      true,
    );
  }, [scheme, revision, spec, table, hidden, inspected, startLabel, endLabel]);
  return (
    <>
      {legend.length > 0 && (
        <div className="visual-answer__controls">
          {legend.map((name) => (
            <button
              type="button"
              key={name}
              aria-pressed={!hidden.includes(name)}
              onClick={() =>
                onLegend(hidden.includes(name) ? hidden.filter((m) => m !== name) : [...hidden, name])
              }
            >
              {name}
            </button>
          ))}
        </div>
      )}
      <div
        ref={ref}
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            setCursor(null);
            chartRef.current?.dispatchAction({ type: 'hideTip' });
            return;
          }
          const next =
            e.key === 'Home'
              ? 0
              : e.key === 'End'
                ? inspected.length - 1
                : e.key === 'ArrowRight' || e.key === 'ArrowDown'
                  ? Math.min(inspected.length - 1, (cursor ?? -1) + 1)
                  : e.key === 'ArrowLeft' || e.key === 'ArrowUp'
                    ? Math.max(0, (cursor ?? 1) - 1)
                    : null;
          if (next === null) return;
          e.preventDefault();
          setCursor(next);
          const entry = inspected[next];
          if (entry && 'row' in entry && typeof entry.row === 'number') onSelect?.(entry.row);
        }}
        role="img"
        aria-label={title}
        style={{ width: '100%', height: height ?? (spec.kind === 'sparkline' ? 80 : 300) }}
      />
      {inspect && <output aria-live="polite">{inspect.text}</output>}
    </>
  );
}
