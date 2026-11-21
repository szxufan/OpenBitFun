export interface StaticHtmlSource {
  html: string;
  css?: string;
  script?: string;
}

export interface StaticHtmlDesign {
  revision: number;
  label: string;
  styleResources: string[];
  viewport: { width?: number; height?: number };
}

const ALLOWED_ELEMENTS = new Set([
  'html', 'head', 'body', 'title', 'style',
  'main', 'section', 'article', 'header', 'footer', 'aside', 'nav', 'div', 'span',
  'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'p', 'br', 'hr', 'strong', 'em', 'b', 'i', 'small', 'sub', 'sup', 'code', 'pre', 'blockquote',
  'ul', 'ol', 'li', 'dl', 'dt', 'dd',
  'figure', 'figcaption', 'details', 'summary',
  'label', 'fieldset', 'legend', 'button', 'input', 'select', 'option', 'textarea',
  'table', 'caption', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td',
  'a', 'img', 'canvas', 'video', 'audio',
  'svg', 'g', 'path', 'circle', 'ellipse', 'line', 'polyline', 'polygon', 'rect', 'defs', 'lineargradient', 'radialgradient', 'stop', 'clippath', 'mask', 'pattern', 'symbol', 'text', 'tspan', 'marker', 'use',
]);

const ALLOWED_ATTRIBUTES = new Set([
  'id', 'class', 'title', 'role', 'tabindex', 'dir', 'lang', 'hidden', 'slot', 'style',
  'href', 'target', 'rel', 'download', 'src', 'alt', 'width', 'height', 'loading', 'decoding',
  'type', 'name', 'value', 'placeholder', 'checked', 'disabled', 'readonly', 'required', 'multiple', 'selected', 'min', 'max', 'step', 'rows', 'cols', 'for', 'autocomplete', 'inputmode', 'accept',
  'viewbox', 'preserveaspectratio', 'fill', 'stroke', 'stroke-width', 'stroke-linecap', 'stroke-linejoin', 'fill-rule', 'clip-rule', 'd', 'cx', 'cy', 'r', 'rx', 'ry', 'x', 'x1', 'x2', 'y', 'y1', 'y2', 'points', 'transform', 'opacity', 'offset', 'stop-color', 'stop-opacity', 'gradientunits', 'gradienttransform', 'patternunits', 'patterncontentunits', 'patterntransform', 'xmlns', 'version',
]);

function sanitizeCss(css: string): string {
  return css
    .replace(/@import[^;]+;?/gi, '')
    .replace(/url\s*\(\s*['"]?(?:https?:|data:|javascript:|vbscript:|\/\/)[^)]*\)/gi, 'none');
}

export function sanitizeStaticHtml(source: StaticHtmlSource, specification?: StaticHtmlDesign, hostStyleResourceCss = '', colorScheme: 'light' | 'dark' = 'light') {
  const diagnostics: string[] = [];
  const parsed = new DOMParser().parseFromString(source.html, 'text/html');
  parsed.querySelectorAll('*').forEach(element => {
    const tagName = element.tagName.toLowerCase();
    if (!ALLOWED_ELEMENTS.has(tagName)) {
      diagnostics.push(`Removed unsupported element: <${tagName}>`);
      element.remove();
      return;
    }
    for (const attribute of [...element.attributes]) {
      const attributeName = attribute.name.toLowerCase();
      if (attributeName.startsWith('on') || (!ALLOWED_ATTRIBUTES.has(attributeName) && !attributeName.startsWith('aria-') && !attributeName.startsWith('data-'))) {
        diagnostics.push(`Removed unsupported attribute: ${attribute.name}`);
        element.removeAttribute(attribute.name);
        continue;
      }
      if (attributeName === 'style') {
        element.setAttribute(attribute.name, sanitizeCss(attribute.value));
      }
      if ((attributeName === 'href' || attributeName === 'src' || attributeName === 'xlink:href')
        && /^(?:https?:|data:|javascript:|vbscript:|\/\/)/i.test(attribute.value.trim())) {
        diagnostics.push(`Removed unsafe URL from ${attribute.name}`);
        element.removeAttribute(attribute.name);
      }
    }
  });
  parsed.querySelectorAll('style').forEach(style => {
    style.textContent = sanitizeCss(style.textContent ?? '');
  });
  const css = sanitizeCss(source.css ?? '');
  if (css.trim()) {
    const style = parsed.createElement('style');
    style.setAttribute('data-gen-ui-source-style', 'true');
    style.textContent = css;
    parsed.head.appendChild(style);
  }
  if (specification) {
    const identity = parsed.createElement('meta');
    identity.name = 'openbitfun-design-spec';
    identity.content = `${specification.label}@${specification.revision}`;
    parsed.head.appendChild(identity);
    const resources = parsed.createElement('meta');
    resources.name = 'openbitfun-style-resources';
    resources.content = specification.styleResources.join(',');
    parsed.head.appendChild(resources);
  }
  if (hostStyleResourceCss.trim()) {
    const style = parsed.createElement('style');
    style.setAttribute('data-gen-ui-host-style-resources', 'true');
    style.textContent = hostStyleResourceCss;
    parsed.head.appendChild(style);
  }
  const policy = parsed.createElement('meta');
  policy.httpEquiv = 'Content-Security-Policy';
  policy.content = `default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; script-src 'none'`;
  parsed.head.prepend(policy);
  if (source.script?.trim()) diagnostics.push('Controlled script execution is disabled on this host');
  return {
    document: `<!doctype html><html data-openbitfun-appearance-mode="${colorScheme}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${parsed.head.innerHTML}</head><body>${parsed.body.innerHTML}</body></html>`,
    diagnostics,
  };
}
