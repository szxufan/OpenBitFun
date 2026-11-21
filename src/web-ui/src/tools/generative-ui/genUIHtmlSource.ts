import type { GenUIDesignSpec } from './genUIContract';
import type { GenUIHtmlDocumentSource } from './genUITypes';
import { sanitizeStaticHtml } from '@/shared/ui/static-document/sanitizeStaticHtml';

export function inspectGenUIHtmlSource(source: GenUIHtmlDocumentSource, specification?: GenUIDesignSpec): string[] {
  const { diagnostics } = sanitizeStaticHtml(source);
  const issues = diagnostics.filter(message => message.startsWith('Removed unsafe URL'));
  if (specification && !specification.sourceKinds.includes('html-document')) issues.push(`UI specification ${specification.label} does not allow HTML documents`);
  const parsed = new DOMParser().parseFromString(source.html, 'text/html');
  for (const selector of specification?.shell.forbiddenContentSelectors ?? []) {
    try {
      if (parsed.querySelector(selector)) issues.push('Generated document duplicates host-owned shell part: ' + selector);
    } catch {
      issues.push('Invalid host-owned content selector in UI specification: ' + selector);
    }
  }
  // Reject the unconditionally empty case before mounting an opaque iframe.
  // Actual browser inspection still owns visibility, geometry and overflow
  // checks for documents that contain content but may be hidden or misplaced
  // by their CSS.
  const body = parsed.body;
  const hasBodyText = Boolean(body?.textContent?.trim());
  const hasRenderableElement = Boolean(body?.querySelector('svg,img,canvas,video,audio,input,button,select,textarea,[role],iframe'));
  if (!hasBodyText && !hasRenderableElement) issues.push('HTML document has no visible rendered content');
  return issues;
}
