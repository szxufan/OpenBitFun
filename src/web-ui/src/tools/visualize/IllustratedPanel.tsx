import { Fragment } from 'react';
import type { WidgetOf } from './widgets';
import { diagramArtwork } from './diagramArtwork';
import './IllustratedPanel.scss';

type Panel = WidgetOf<'tabs'>['panels'][number];

function FlowDiagram({ nodes }: { nodes: NonNullable<Panel['nodes']> }) {
  return (
    <ol data-openbitfun-component="gen-ui-components" data-openbitfun-part="layout" className="visual-guide__diagram" data-illustrated={nodes.some(node => node.icon || node.drawing) || undefined}>
      {nodes.map((node, i) => (
        <li key={i} className="visual-guide__node">
          {(node.icon || node.drawing) && (
            <svg data-openbitfun-component="gen-ui-components" data-openbitfun-part="media" viewBox="0 0 146 95" focusable="false" aria-hidden="true">
              {node.drawing
                ? node.drawing.map((path, j) => <path key={j} d={path.d} className={`visual-guide__${path.tone}`} />)
                : node.icon && diagramArtwork[node.icon]}
            </svg>
          )}
          <div>
            <h3>{node.title}</h3>
            {node.caption && <p>{node.caption}</p>}
          </div>
        </li>
      ))}
    </ol>
  );
}

export function IllustratedPanel({ panel, selection, onChange }: {
  panel: Panel;
  selection?: string;
  onChange: (index: number) => void;
}) {
  const index = Number(selection ?? 0);
  const selected = Number.isInteger(index) ? panel.variants?.[index] : undefined;
  const variant = selected ?? panel.variants?.[0];
  // A fixed diagram introduces the controls. When a choice changes the
  // diagram, put the controls first so the changing content stays below it.
  const nodes = variant?.nodes ?? panel.nodes;
  const changesDiagram = panel.variants?.some(v => v.nodes);
  return (
    <>
      {nodes && !changesDiagram && <FlowDiagram nodes={nodes} />}
      {panel.outline && (
        <ol data-openbitfun-component="gen-ui-components" data-openbitfun-part="content" className="visual-guide__outline">
          {panel.outline.map((row, i) => (
            <li key={i} data-highlight={row.highlight || undefined}>
              <div><h3>{row.title}</h3>{row.path && <code>{row.path}</code>}</div>
              <p>{row.summary}</p>
            </li>
          ))}
        </ol>
      )}
      {panel.variants && (
        <div role="group" aria-label={panel.title} data-openbitfun-component="gen-ui-components" data-openbitfun-part="control" className="visual-guide__variants">
          {panel.variants.map((choice, i) => (
            <button key={i} type="button" aria-pressed={choice === variant} onClick={() => onChange(i)}>
              {choice.label}
            </button>
          ))}
        </div>
      )}
      {variant && (
        <div data-openbitfun-component="gen-ui-components" data-openbitfun-part="content" className="visual-guide__result" aria-live="polite" aria-atomic="true">
          {changesDiagram && nodes && <FlowDiagram nodes={nodes} />}
          {variant.steps && (
            <p className="visual-guide__steps">
              {variant.steps.map((step, i) => (
                <Fragment key={i}>{i > 0 && <span aria-hidden="true"> → </span>}{step}</Fragment>
              ))}
            </p>
          )}
          {variant.detail && <p>{variant.detail}</p>}
        </div>
      )}
    </>
  );
}
