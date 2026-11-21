import { useMemo, useRef } from 'react';
import { useHostAppearance } from '@/infrastructure/appearance/hooks/useHostAppearance';
import { sanitizeStaticHtml, type StaticHtmlDesign, type StaticHtmlSource } from './sanitizeStaticHtml';
import './StaticHtmlDocument.scss';

export default function StaticHtmlDocument({ source, specification }: {
  source: StaticHtmlSource;
  specification?: StaticHtmlDesign;
}) {
  const themeRef = useRef<HTMLDivElement>(null);
  const appearance = useHostAppearance(themeRef);
  const hostStyleResourceCss = useMemo(() => {
    if (typeof document === 'undefined') return '';
    const computed = getComputedStyle(themeRef.current ?? document.documentElement);
    const tokens: string[] = [];
    for (let index = 0; index < computed.length; index++) {
      const name = computed.item(index);
      if (!name.startsWith('--openbitfun-') || name.startsWith('--openbitfun-skin-')) continue;
      const value = computed.getPropertyValue(name).trim();
      // Appearance values are host-owned, but do not copy malformed values
      // into the opaque document's inline stylesheet.
      if (value && !/[{};]/.test(value)) tokens.push(`${name}:${value};`);
    }
    return `:root{color-scheme:${appearance.scheme};${tokens.join('')}}html,body{margin:0;min-height:100%;color:var(--openbitfun-color-content-primary);background:var(--openbitfun-color-surface-canvas);font-family:var(--openbitfun-type-body-md-font-family);font-size:var(--openbitfun-type-body-md-font-size);line-height:var(--openbitfun-type-body-md-line-height);}`;
  }, [appearance]);
  const sanitized = useMemo(() => sanitizeStaticHtml(source, specification, hostStyleResourceCss, appearance.scheme),
    [source, specification, hostStyleResourceCss, appearance.scheme]);
  return <div ref={themeRef} className="gen-ui-html-document" data-openbitfun-component="generative-widget" data-openbitfun-part="app" data-gen-ui-html-document="true" data-gen-ui-source-kind="html-document" data-gen-ui-sanitize-diagnostics={sanitized.diagnostics.length}>
    <iframe
      title="Generated HTML document"
      className="gen-ui-html-document__frame"
      sandbox=""
      referrerPolicy="no-referrer"
      srcDoc={sanitized.document}
      data-gen-ui-html-frame="true"
      data-gen-ui-viewport-width={specification?.viewport.width}
      data-gen-ui-viewport-height={specification?.viewport.height}
    />
  </div>;
}
