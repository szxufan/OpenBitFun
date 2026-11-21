// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { sanitizeStaticHtml } from './sanitizeStaticHtml';

describe('read-only HTML documents', () => {
  it('passes the active Appearance mode into the isolated document', () => {
    const result = sanitizeStaticHtml(
      { html: '<main>Theme aware</main>' },
      undefined,
      ':root{color-scheme:dark;--openbitfun-color-content-primary:#f3f3f5}',
      'dark',
    );
    expect(result.document).toContain('<html data-openbitfun-appearance-mode="dark">');
    expect(result.document).toContain('color-scheme:dark');
    expect(result.document).toContain('--openbitfun-color-content-primary:#f3f3f5');
  });

  it('keeps HTML/SVG content while removing active and unsafe host paths', () => {
    const result = sanitizeStaticHtml({
      html: '<section onclick="alert(1)"><svg viewBox="0 0 10 10"><a href="javascript:alert(1)"><path /></a></svg><script>window.bad = true</script></section>',
      css: '@import url(https://example.invalid/style.css); section { width: 100%; }',
      script: 'window.openbitfunWidget.send({ kind: "event" })',
    });
    expect(result.document).toContain('<svg');
    expect(result.document).not.toContain('<script');
    expect(result.document).not.toContain('onclick');
    expect(result.document).not.toContain('javascript:');
    expect(result.document).toContain('section { width: 100%; }');
    expect(result.document).toContain("Content-Security-Policy");
    expect(result.document).toContain("script-src 'none'");
    expect(result.diagnostics).toContain('Controlled script execution is disabled on this host');
  });

});
