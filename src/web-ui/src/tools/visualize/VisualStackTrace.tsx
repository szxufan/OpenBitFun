import { useI18n } from '@/infrastructure/i18n';
import type { VisualTable } from './document';

export function VisualStackTrace({
  columns,
  rows,
  selected,
  select,
}: VisualTable & {
  selected: number;
  select: (row: number) => void;
}) {
  const { formatNumber } = useI18n('flow-chat');
  const [name, file, line, source] = rows[selected];
  return (
    <>
      <ol className="visual-answer__frames" aria-label={columns[0]}>
        {rows.map(([fn, path, lineNumber], index) => (
          <li key={index}>
            <button type="button" aria-pressed={index === selected} onClick={() => select(index)}>
              {String(fn)} · {String(path)}:{formatNumber(Number(lineNumber), { useGrouping: false })}
            </button>
          </li>
        ))}
      </ol>
      <div aria-live="polite">
        <strong>{String(name)}</strong>{' '}
        <code>
          {String(file)}:{formatNumber(Number(line), { useGrouping: false })}
        </code>
        {typeof source === 'string' && (
          <pre>
            <code>{source}</code>
          </pre>
        )}
      </div>
    </>
  );
}
