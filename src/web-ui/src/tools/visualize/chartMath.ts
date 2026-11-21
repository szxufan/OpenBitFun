import type { VisualSpec, VisualTable } from './document';
type Rows = VisualTable['rows'];
export function histogram(rows: Rows, statistic = 'count') {
  if (rows[0].length === 4) {
    const total = rows.reduce((n, r) => n + Number(r[3]), 0);
    return rows.map(([a, b, label, count]) => ({
      a: Number(a),
      b: Number(b),
      label: String(label),
      count: Number(count),
      height:
        Number(count) /
        (statistic === 'count' ? 1 : (Number(b) - Number(a)) * (statistic === 'density' ? total : 1)),
    }));
  }
  const values = rows.map((r) => Number(r[0])),
    min = Math.min(...values),
    max = Math.max(...values);
  const size = min === max ? 1 : Math.ceil(Math.sqrt(rows.length)),
    width = (max - min) / size || 1;
  const bins = Array.from({ length: size }, (_, i) => ({
    a: min + i * width,
    b: min + (i + 1) * width,
    label: '',
    count: 0,
    height: 0,
  }));
  for (const v of values) {
    const bin = bins[Math.min(size - 1, Math.floor((v - min) / width))];
    bin.count++;
    bin.height++;
  }
  return bins;
}
export function boxStatistics(
  rows: Rows,
  whiskers: NonNullable<VisualSpec['options']>['whiskers'] = 'tukey',
) {
  const groups = new Map<string, number[]>();
  for (const [label, v] of rows) {
    const name = String(label),
      group = groups.get(name) ?? [];
    group.push(Number(v));
    groups.set(name, group);
  }
  return [...groups].map(([label, values]) => {
    values.sort((a, b) => a - b);
    const q = (p: number) => {
      const i = (values.length - 1) * p,
        f = Math.floor(i);
      return values[f] + (values[Math.ceil(i)] - values[f]) * (i - f);
    };
    const q1 = q(0.25),
      q3 = q(0.75),
      iqr = q3 - q1;
    const low = Array.isArray(whiskers)
      ? Math.min(q1, values.find((v) => v >= q(whiskers[0])) ?? q1)
      : whiskers === 'minMax'
        ? values[0]
        : values.find((v) => v >= q1 - 1.5 * iqr)!;
    const high = Array.isArray(whiskers)
      ? Math.max(q3, [...values].reverse().find((v) => v <= q(whiskers[1])) ?? q3)
      : whiskers === 'minMax'
        ? values.at(-1)!
        : [...values].reverse().find((v) => v <= q3 + 1.5 * iqr)!;
    return {
      label,
      values: [low, q1, q(0.5), q3, high],
      outliers: values.filter((v) => v < low || v > high),
    };
  });
}
// Solve least squares in centered, scaled coordinates to avoid squaring timestamps.
export function fitPoints(rows: Rows, degree: 1 | 2) {
  const xs = rows.map((r) => Number(r[0])),
    min = Math.min(...xs),
    max = Math.max(...xs);
  if (new Set(xs).size <= degree) return [];
  const x = (v: number) => ((v - min) / (max - min)) * 2 - 1,
    n = degree + 1;
  const matrix = Array.from({ length: n }, (_, i) =>
    Array.from({ length: n + 1 }, (_, j) =>
      rows.reduce(
        (a, r) => a + Math.pow(x(Number(r[0])), i) * (j === n ? Number(r[1]) : Math.pow(x(Number(r[0])), j)),
        0,
      ),
    ),
  );
  for (let j = 0; j < n; j++) {
    let pivot = j;
    for (let i = j + 1; i < n; i++) if (Math.abs(matrix[i][j]) > Math.abs(matrix[pivot][j])) pivot = i;
    [matrix[j], matrix[pivot]] = [matrix[pivot], matrix[j]];
    const d = matrix[j][j];
    if (Math.abs(d) < 1e-12) return [];
    matrix[j] = matrix[j].map((v) => v / d);
    for (let i = 0; i < n; i++)
      if (i !== j) {
        const f = matrix[i][j];
        matrix[i] = matrix[i].map((v, k) => v - f * matrix[j][k]);
      }
  }
  return Array.from({ length: 33 }, (_, i) => {
    const v = min + ((max - min) * i) / 32;
    return [v, matrix.reduce((a, r, j) => a + r[n] * Math.pow(x(v), j), 0)];
  });
}
export function stagedFlows(rows: Rows) {
  const nodes = new Map<string, { name: string; depth: number; label: { formatter: string } }>(),
    links = new Map<string, { source: string; target: string; value: number }>();
  for (const row of rows) {
    for (let i = 0; i < row.length - 1; i++) {
      const name = JSON.stringify([i, row[i]]);
      nodes.set(name, { name, depth: i, label: { formatter: String(row[i]).replace(/[{}]/g, '') } });
      if (!i) continue;
      const source = JSON.stringify([i - 1, row[i - 1]]),
        key = JSON.stringify([source, name]);
      const link = links.get(key) ?? { source, target: name, value: 0 };
      link.value += Number(row.at(-1));
      links.set(key, link);
    }
  }
  return { data: [...nodes.values()], links: [...links.values()] };
}
