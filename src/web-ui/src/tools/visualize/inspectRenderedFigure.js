// This script starts before the fragment executes. Return evidence over a private
// port so application messages and authored postMessage traffic cannot finish a check.
(() => {
  const channel = new MessageChannel();
  const reply = channel.port1;
  const issues = [];
  const bound = (e) => {
    const r = e.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden';
  };
  function error(event) {
    const el = event.target;
    if (el instanceof HTMLLinkElement && ['preload', 'modulepreload'].includes(el.rel)) return;
    if (event.message || el?.matches?.('script[src], link[rel=stylesheet], img[src]')) {
      issues.push(
        String(event.message || 'Could not load ' + (el.getAttribute('src') || el.href)).slice(0, 240),
      );
    }
  }
  const rejection = (e) => issues.push(String(e.reason?.message || e.reason).slice(0, 240));
  addEventListener('error', error, true);
  addEventListener('unhandledrejection', rejection);
  parent.postMessage({ openBitFunVisual: 2, type: 'inspect-render' }, '*', [channel.port2]);
  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  function sampleCanvas(el) {
    const probe = document.createElement('canvas');
    probe.width = probe.height = 64;
    const ctx = probe.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(el, 0, 0, 64, 64);
    return ctx.getImageData(0, 0, 64, 64).data.join(',');
  }
  function motion(el) {
    if (el instanceof HTMLCanvasElement) return sampleCanvas(el);
    const r = el.getBoundingClientRect();
    return [
      el.outerHTML,
      ...[el, ...el.querySelectorAll('*')].map((e) => getComputedStyle(e).transform),
      r.x,
      r.y,
      r.width,
      r.height,
    ].join('|');
  }
  // On new writes the map destination carries the author's intent even if its
  // fragment lost the atlas declaration. Also inspect the accessible name of
  // an existing free-form route drawing. Historical replay uses its old check.
  function inspectMapPublication({ mapExpected, mapRequested, itineraryExpected }) {
    const mapRepair = 'Missing geographic basemap. Use data-visual-atlas="1" with detail:"street", city, country and the public stop names for a local itinerary. ' +
      (mapRequested ? 'The reader asked for a map. If publication fails, report the specific error.' :
        'If street data is unavailable, show a labeled stop list without a scale.');
    let found = false;
    let cityOnly = false;
    let hasScale = false;
    for (const el of document.querySelectorAll('svg,canvas,img,[data-visual-map]')) {
      if (!bound(el) || el.parentElement?.closest('svg,[data-visual-map]')) continue;
      const title = el.getAttribute('aria-label') || el.getAttribute('alt') || el.querySelector('title')?.textContent || '';
      const scale = el.querySelector('[data-map-scale]') || [...el.querySelectorAll('text')].some(
        text => /^\s*\d+(?:[.,]\d+)?\s*(?:km|mi|千米|公里|英里)\s*$/iu.test(text.textContent));
      if (scale) hasScale = true;
      const route = /\b(?:route|itinerary|street|walking|geographic)\s+map\b|地[图圖]|[路航][线線](?:示意)?[图圖]/iu.test(title);
      const declared = el.hasAttribute('data-visual-map');
      if (!declared && !route && !mapExpected) continue;
      // A labeled, unscaled schematic may accompany an itinerary. It does not
      // satisfy a requested geographic map.
      if (!declared && !mapExpected && !scale && /schematic|示意/iu.test(title)) continue;
      const source = el.getAttribute('data-map-source') || '';
      const geometry = el.querySelectorAll('[data-map-layer], [data-street-layer], [data-visual-region]');
      if ((el.hasAttribute('data-visual-atlas-map') || /^https?:\/\//i.test(source)) &&
          [...geometry].some(shape => shape instanceof SVGGeometryElement && bound(shape) && shape.getTotalLength() > 0)) {
        if (itineraryExpected && el.hasAttribute('data-visual-atlas-map') && el.querySelector('[data-map-layer="land"]')) {
          const positions = Array.from(el.querySelectorAll('[data-map-stop] circle'), dot =>
            dot.getAttribute('cx') + ',' + dot.getAttribute('cy'));
          if (new Set(positions).size === 1) { cityOnly = true; continue; }
        }
        found = true;
      } else if (declared || route) return mapRepair;
    }
    if (cityOnly && !found) return 'This itinerary map shows only a city center. Plot the public stops in visit order using detail:"street", city and places.';
    if (mapExpected && !found) {
      const stops = document.querySelector('ol[data-visual-itinerary]');
      if (mapRequested || !stops || !bound(stops) || !stops.querySelector('li') || hasScale || [...document.querySelectorAll('[data-map-scale]')].some(bound)) return mapRepair;
    }
    return '';
  }
  function inspect(publication) {
    if (issues.length) return issues[0];
    if (document.readyState === 'loading') return 'Markup is still loading';
    if (publication && typeof publication === 'object') {
      const problem = inspectMapPublication(publication);
      if (problem) return problem;
    }
    const map = document.querySelector('[data-visual-map]');
    if (map) {
      const shapes = [...map.querySelectorAll('[data-visual-region], path[d], polygon[points]')];
      if (
        !shapes.some(
          (el) => bound(el) && (el instanceof SVGGeometryElement ? el.getTotalLength() > 0 : false),
        )
      ) {
        return 'The map has no visible projected geometry. Embed the basemap and check its projection.';
      }
      for (const marker of map.querySelectorAll('[data-visual-marker]')) {
        if (!bound(marker)) return 'A required map marker or label is hidden or unprojected.';
      }
    }
    for (const e of document.body.children) {
      if (
        !e.matches('script,style,link,template,noscript') &&
        bound(e) &&
        (e.innerText?.trim() ||
          e.matches('svg,canvas,img,input,button') ||
          e.childElementCount ||
          getComputedStyle(e).backgroundImage !== 'none' ||
          e.shadowRoot)
      )
        return '';
    }
    return 'The fragment produced no visible content.';
  }
  // Exercise the change handler in the disposable publishing frame. Reading a
  // saved figure must never change the reader's selection or persisted state.
  async function checkSelects() {
    for (const select of document.querySelectorAll('select:enabled:not([multiple])')) {
      if (!bound(select)) continue;
      const option = Array.from(select.options).find(o =>
        o.value !== select.value && !o.hidden && !o.matches(':disabled'));
      if (!option) continue;
      select.selectedIndex = option.index;
      select.dispatchEvent(new Event('input', { bubbles: true }));
      select.dispatchEvent(new Event('change', { bubbles: true }));
      await delay(0);
      if (issues.length) return 'Selecting "' + option.label.slice(0, 60) + '" failed: ' + issues[0];
    }
  }
  async function run(publication) {
    let issue;
    const start = Date.now();
    do {
      await delay(100);
      issue = inspect(publication);
    } while (issue && !issues.length && Date.now() - start < 5500);
    if (!issue && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
      const movers = [...document.querySelectorAll('[data-visual-motion]')].filter(bound);
      if (movers.length) {
        const first = movers.map(motion);
        let moving = false;
        for (const ms of [135, 310, 700]) {
          await delay(ms);
          if (movers.some((el, i) => motion(el) !== first[i])) {
            moving = true;
            break;
          }
        }
        if (!moving)
          issue = 'The marked simulation is stationary. Check its animation target and startup state.';
      }
    }
    if (!issue && !issues.length && publication && typeof publication === 'object' && !publication.replay) {
      const selectionError = await checkSelects();
      if (selectionError) return selectionError;
    }
    return issues[0] || issue || null;
  }
  reply.onmessage = async ({ data }) => {
    reply.onmessage = null;
    try {
      reply.postMessage(await run(data));
    } catch (e) {
      reply.postMessage(String(e?.message || e).slice(0, 240));
    } finally {
      removeEventListener('error', error, true);
      removeEventListener('unhandledrejection', rejection);
      reply.close();
    }
  };
})();
