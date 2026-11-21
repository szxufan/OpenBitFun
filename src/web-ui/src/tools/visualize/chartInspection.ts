import { boxStatistics, histogram } from './chartMath';
import type { VisualSpec, VisualTable } from './document';
import { i18nService } from '@/infrastructure/i18n';

export function chartInspection(spec: VisualSpec, table: VisualTable, hidden: string[] = []) {
  const { rows, columns } = table;
  const number = (n: number) =>
    i18nService.formatNumber(n, spec.options?.valueFormat ?? { maximumSignificantDigits: 8 });
  const percent = (n: number) => i18nService.formatNumber(n, { style: 'percent', maximumFractionDigits: 2 });
  const cell = (v: string | number | null) => (typeof v === 'number' ? number(v) : (v ?? '—'));
  if (spec.kind === 'histogram')
    return histogram(rows, spec.options?.statistic).map((b) => ({
      text: [
        b.label || number(b.a) + '–' + number(b.b),
        number(b.count),
        ...(spec.options?.statistic ? [spec.options.statistic + ' = ' + number(b.height)] : []),
      ].join(' · '),
    }));
  if (spec.kind === 'box-plot' && columns.length === 2)
    return boxStatistics(rows, spec.options?.whiskers).map((g) => ({
      text: [g.label, ...g.values.map(number), ...g.outliers.map(number)].join(' · '),
    }));
  const total = spec.kind === 'pie' ? rows.reduce((sum, r) => sum + Number(r[1]), 0) : 0;
  return rows.map((r, i) => {
    const fields = r.map((v, j) => columns[j] + ' = ' + cell(v));
    if (spec.stacked === 'percent') {
      const sum = r
        .slice(1)
        .reduce<number>((n, v, j) => n + (hidden.includes(columns[j + 1]) ? 0 : Number(v ?? 0)), 0);
      for (let j = 1; j < r.length; j++)
        if (!hidden.includes(columns[j]) && r[j] !== null && sum > 0)
          fields[j] += ' (' + percent(Number(r[j]) / sum) + ')';
    }
    for (const [label, value] of spec.options?.rowDetails?.[i] ?? [])
      fields.push(label + ' = ' + String(value ?? '—'));
    if (spec.kind === 'pie' && total) fields.push(percent(Number(r[1]) / total));
    if (spec.kind === 'funnel' && r[1] !== null) {
      if (Number(rows[0][1]) > 0) fields.push(percent(Number(r[1]) / Number(rows[0][1])));
      if (i > 0 && Number(rows[i - 1][1]) > 0)
        fields.push('Δ ' + percent(Number(r[1]) / Number(rows[i - 1][1]) - 1));
    }
    return { text: fields.join(' · '), row: i };
  });
}
