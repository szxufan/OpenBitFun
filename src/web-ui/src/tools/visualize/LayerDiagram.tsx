import { useId } from 'react';
import type { WidgetOf } from './widgets';
import './LayerDiagram.scss';

export function LayerDiagram({ data, title, selected, onSelect }: {
  data: WidgetOf<'layers'>;
  title: string;
  selected: string | undefined;
  onSelect: (index: string) => void;
}) {
  const group = useId();
  const index = Number(selected ?? 0);
  const current = Number.isInteger(index) && data.layers[index] ? index : 0;
  const layer = data.layers[current];

  return (
    <div className="visual-layers__body" data-openbitfun-component="gen-ui-components" data-openbitfun-part="layout">
      <div data-openbitfun-component="gen-ui-components" data-openbitfun-part="control" className="visual-layers__stack" role="radiogroup" aria-label={title}>
        {data.layers.map((entry, i) => (
          <label key={i} className="visual-layers__choice" data-selected={i === current || undefined}>
            <input type="radio" name={group} checked={i === current} onChange={() => onSelect(String(i))} />
            <svg data-openbitfun-component="gen-ui-components" data-openbitfun-part="media" aria-hidden="true" viewBox="0 0 156 57" focusable="false">
              <path className="visual-layers__side" d="M5 30 55 47 150 23 150 29 55 53 5 37Z" />
              <path className="visual-layers__top" d="M5 30 100 7 150 23 55 47Z" />
              <path className="visual-layers__fold" d="M55 47V53" />
            </svg>
            <span>{entry.name}</span>
          </label>
        ))}
      </div>
      <div data-openbitfun-component="gen-ui-components" data-openbitfun-part="content" className="visual-layers__detail" aria-live="polite" aria-atomic="true">
        <h3>{layer.name}</h3>
        <p>{layer.summary}</p>
        {layer.path && <code className="visual-layers__path">{layer.path}</code>}
        {!!layer.modules?.length && (
          <ul>{layer.modules.map((name, i) => <li key={i}><code>{name}</code></li>)}</ul>
        )}
      </div>
    </div>
  );
}
