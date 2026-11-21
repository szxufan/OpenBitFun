import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { ImageIcon } from 'lucide-react';
import { i18nService } from '@/infrastructure/i18n';
import type { GenUIComponentProps } from '../genUILibrary';
import { str, safeUrl, inlineText, color, length, radius, space, chartColor } from './oiComponentHelpers';

export function Layout({ view, slots }: GenUIComponentProps) {
  const props = view.props;
  const direction = props.direction === 'row' ? 'row' : 'column';
  const style: CSSProperties = { gap: space(props.gap), padding: space(props.padding), alignItems: props.align === 'start' ? 'flex-start' : props.align === 'end' ? 'flex-end' : props.align === 'center' ? 'center' : props.align === 'stretch' ? 'stretch' : undefined, justifyContent: props.justify === 'between' ? 'space-between' : props.justify === 'start' ? 'flex-start' : props.justify === 'end' ? 'flex-end' : props.justify === 'center' ? 'center' : undefined, flexWrap: props.wrap ? 'wrap' : undefined, background: color(props.background), borderRadius: radius(props.radius), overflow: props.radius !== undefined ? 'clip' : undefined, border: props.border ? '1px solid var(--openbitfun-color-border-default)' : undefined, width: length(props.width), height: length(props.height), flex: typeof props.flex === 'number' ? `${Math.max(0, props.flex)} 1 0%` : typeof props.width === 'number' ? '0 0 auto' : undefined };
  return <div className={`gen-ui-oi-box ${direction === 'row' ? 'gen-ui-oi-row' : 'gen-ui-oi-col'} ${direction === 'column' && props.gap == null ? 'gen-ui-oi-flow' : ''}`} data-openbitfun-component="gen-ui-components" data-openbitfun-part="layout" style={style}>{slots.children}</div>;
}

export function ImageView({ view }: Pick<GenUIComponentProps, 'view'>) {
  const alt = str(view.props.alt ?? view.props.query);
  const src = safeUrl(str(view.props.src));
  const [failed, setFailed] = useState<string>();
  const aspectRatio = typeof view.props.aspectRatio === 'number' && view.props.aspectRatio > 0.2 && view.props.aspectRatio < 5 ? view.props.aspectRatio : 1.5;
  return <figure className="gen-ui-oi-image" data-openbitfun-component="gen-ui-components" data-openbitfun-part="media" style={{ aspectRatio, borderRadius: radius(view.props.radius ?? 'lg') }}>
    {src && src !== failed ? <img src={src} alt={alt} loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(src)} style={{ objectFit: view.props.fit === 'contain' ? 'contain' : 'cover' }} /> : <div className="gen-ui-oi-image-empty" role="img" aria-label={alt}><ImageIcon aria-hidden strokeWidth={1.5} /></div>}
    <figcaption>{view.props.caption ? inlineText(str(view.props.caption)) : alt}</figcaption>
  </figure>;
}

type ChartRow = Record<string, unknown>;
const chartNumber = (value: unknown) => typeof value === 'number' && Number.isFinite(value) ? value : Number(value) || 0;
function chartFormat(value: number, kind: unknown, currency: unknown, compact = false) {
  if (kind === 'percent') return `${Number(value.toFixed(compact ? 1 : 2))}%`;
  const code = typeof currency === 'string' && /^[A-Za-z]{3}$/.test(currency) ? currency.toUpperCase() : 'USD';
  const options: Intl.NumberFormatOptions = { maximumFractionDigits: compact ? 1 : 2, notation: compact && Math.abs(value) >= 1000 ? 'compact' : 'standard' };
  return i18nService.formatNumber(value, kind === 'currency' ? { ...options, style: 'currency', currency: code } : options);
}
function chartAxis(values: number[]) {
  const min = Math.min(0, ...values), max = Math.max(1, ...values), step = (max - min || 1) / 4;
  return { min, max, step };
}
export function ChartView({ view }: Pick<GenUIComponentProps, 'view'>) {
  const containerRef = useRef<HTMLElement>(null);
  const [containerWidth, setContainerWidth] = useState(640);
  useLayoutEffect(() => {
    const measure = () => {
      const measured = containerRef.current?.clientWidth;
      if (measured) setContainerWidth(Math.max(280, Math.min(720, measured)));
    };
    measure();
    if (typeof ResizeObserver === 'undefined' || !containerRef.current) return;
    const observer = new ResizeObserver(measure);
    observer.observe(containerRef.current);
    return () => observer.disconnect();
  }, []);
  const data = Array.isArray(view.props.data) ? view.props.data.filter((row): row is ChartRow => Boolean(row && typeof row === 'object' && !Array.isArray(row))) : [];
  const type = ['bar', 'line', 'area', 'pie', 'donut', 'scatter'].includes(str(view.props.type)) ? str(view.props.type) : 'bar';
  const first = data[0] ?? {};
  const keys = Object.keys(first);
  const x = typeof view.props.x === 'string' && keys.includes(view.props.x) ? view.props.x : keys[0] ?? 'x';
  const series = Array.isArray(view.props.series) && view.props.series.length ? view.props.series.map(String) : keys.filter(key => key !== x && data.some(row => typeof row[key] === 'number'));
  const stacked = view.props.stacked === true && type !== 'line';
  const width = containerWidth;
  const height = typeof view.props.height === 'number' ? Math.min(480, Math.max(150, view.props.height)) : 240;
  const [hover, setHover] = useState<number | null>(null);
  if (!data.length || !series.length) return <figure ref={containerRef} className="gen-ui-oi-chart" data-chart-type={type}><div style={{ minHeight: height }} /></figure>;
  if (type === 'pie' || type === 'donut') {
    const valueKey = series[0];
    const values = data.map(row => Math.max(0, chartNumber(row[valueKey])));
    const total = values.reduce((sum, value) => sum + value, 0) || 1;
    const cx = 130, cy = 120, radius = 92, inner = type === 'donut' ? 54 : 0;
    let angle = -Math.PI / 2;
    const point = (r: number, a: number) => `${cx + r * Math.cos(a)},${cy + r * Math.sin(a)}`;
    return <figure ref={containerRef} className="gen-ui-oi-chart" data-chart-type={type}><svg viewBox="0 0 260 240" role="img" aria-label={`${type} chart`}>{values.map((value, index) => { const sweep = value / total * Math.PI * 2; const start = angle; angle += sweep; const end = angle; if (sweep > Math.PI * 2 - 0.001) return <circle key={index} cx={cx} cy={cy} r={inner ? (radius + inner) / 2 : radius} fill={inner ? 'none' : chartColor(index)} stroke={inner ? chartColor(index) : undefined} strokeWidth={inner ? radius - inner : undefined} />; const large = sweep > Math.PI ? 1 : 0; const path = inner ? `M${point(radius, start)}A${radius},${radius} 0 ${large} 1 ${point(radius, end)}L${point(inner, end)}A${inner},${inner} 0 ${large} 0 ${point(inner, start)}Z` : `M${cx},${cy}L${point(radius, start)}A${radius},${radius} 0 ${large} 1 ${point(radius, end)}Z`; return <path key={index} d={path} fill={chartColor(index)} stroke="var(--openbitfun-color-surface-panel)" strokeWidth="2"><title>{`${str(data[index][x])}: ${chartFormat(value, view.props.format, view.props.currency)}`}</title></path>; })}</svg><div className="gen-ui-oi-chart-legend">{data.map((row, index) => <span key={index}><i style={{ background: chartColor(index) }} />{str(row[x])} <b>{Math.round(values[index] / total * 1000) / 10}%</b></span>)}</div></figure>;
  }
  const tops = data.map(row => {
    let total = 0;
    return series.map(key => total += Math.max(0, chartNumber(row[key])));
  });
  const allValues = stacked ? tops.map(values => values.at(-1) ?? 0) : data.flatMap(row => series.map(key => chartNumber(row[key])));
  const axis = chartAxis(allValues);
  const pad = { top: 14, right: 12, bottom: 34, left: 58 };
  const plotW = width - pad.left - pad.right, plotH = height - pad.top - pad.bottom;
  const xAt = (index: number) => pad.left + plotW * (data.length === 1 ? 0.5 : index / (data.length - 1));
  const yAt = (value: number) => pad.top + plotH - ((value - axis.min) / (axis.max - axis.min || 1)) * plotH;
  const labelEvery = Math.max(1, Math.ceil(data.length / 8));
  const scatterX = (row: ChartRow, index: number) => typeof row[x] === 'number' ? chartNumber(row[x]) : index;
  const scatterValues = data.map(scatterX);
  const scatterAxis = type === 'scatter' ? chartAxis(scatterValues) : axis;
  const scatterXAt = (value: number) => pad.left + ((value - scatterAxis.min) / (scatterAxis.max - scatterAxis.min || 1)) * plotW;
  return <figure ref={containerRef} className="gen-ui-oi-chart" data-chart-type={type}><svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${type} chart`} onMouseLeave={() => setHover(null)}>{[0, 1, 2, 3, 4].map(tick => { const value = axis.min + (axis.max - axis.min) * tick / 4; return <g key={tick}><line x1={pad.left} x2={width - pad.right} y1={yAt(value)} y2={yAt(value)} className="gen-ui-oi-chart-grid" /><text x={pad.left - 8} y={yAt(value) + 4} textAnchor="end">{chartFormat(value, view.props.format, view.props.currency, true)}</text></g>; })}{data.map((row, index) => index % labelEvery === 0 ? <text key={`label-${index}`} x={type === 'scatter' ? scatterXAt(scatterX(row, index)) : xAt(index)} y={height - 10} textAnchor="middle">{str(row[x])}</text> : null)}{type === 'bar' && data.map((row, index) => { const groupW = plotW / Math.max(1, data.length); const inner = Math.min(groupW * 0.7, 72 * Math.max(1, stacked ? 1 : series.length)); if (stacked) return series.map((key, seriesIndex) => { const top = yAt(tops[index][seriesIndex]); const base = yAt(tops[index][seriesIndex - 1] ?? 0); return <rect key={`${index}-${key}`} x={xAt(index) - inner / 2} y={top} width={inner} height={Math.max(0, base - top)} rx={seriesIndex === series.length - 1 ? 4 : 0} fill={chartColor(seriesIndex)} onMouseEnter={() => setHover(index)} />; }); const barW = Math.max(4, inner / series.length); return series.map((key, seriesIndex) => { const value = chartNumber(row[key]); const xPos = xAt(index) - inner / 2 + seriesIndex * barW; return <rect key={`${index}-${key}`} x={xPos + 1} y={Math.min(yAt(value), yAt(0))} width={Math.max(1, barW - 2)} height={Math.abs(yAt(0) - yAt(value))} rx="4" fill={chartColor(seriesIndex)} onMouseEnter={() => setHover(index)} />; }); })}{(type === 'line' || type === 'area') && series.map((key, seriesIndex) => { const top = (index: number) => stacked ? tops[index][seriesIndex] : chartNumber(data[index][key]); const base = (index: number) => stacked ? (tops[index][seriesIndex - 1] ?? 0) : 0; const points = data.map((_, index) => `${xAt(index)},${yAt(top(index))}`).join(' '); const under = data.map((_, index) => `${xAt(index)},${yAt(base(index))}`).reverse().join(' '); return <g key={key}>{type === 'area' ? <path d={`M${points} L${under} Z`} fill={chartColor(seriesIndex)} opacity={stacked ? '.24' : '.18'} /> : null}<polyline points={points} fill="none" stroke={chartColor(seriesIndex)} strokeWidth="2.5" onMouseEnter={() => setHover(null)} />{data.map((_, index) => <circle key={index} cx={xAt(index)} cy={yAt(top(index))} r={hover === index ? 5 : 3} fill={chartColor(seriesIndex)} onMouseEnter={() => setHover(index)} />)}</g>; })}{type === 'scatter' && series.map((key, seriesIndex) => data.map((row, index) => <circle key={`${key}-${index}`} cx={scatterXAt(scatterX(row, index))} cy={yAt(chartNumber(row[key]))} r={hover === index ? 6 : 4} fill={chartColor(seriesIndex)} onMouseEnter={() => setHover(index)} />))}</svg>{hover !== null ? <div className="gen-ui-oi-chart-tooltip"><strong>{str(data[hover]?.[x])}</strong>{series.map(key => <span key={key}>{key}: {chartFormat(chartNumber(data[hover]?.[key]), view.props.format, view.props.currency)}</span>)}</div> : null}<div className="gen-ui-oi-chart-legend">{series.length > 1 ? series.map((key, index) => <span key={key}><i style={{ background: chartColor(index) }} />{key}</span>) : null}</div></figure>;
}
