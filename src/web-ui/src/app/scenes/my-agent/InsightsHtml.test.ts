// @vitest-environment jsdom

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

const renderer = readFileSync(
  resolve(process.cwd(), '../crates/assembly/core/src/agentic/insights/html.rs'),
  'utf8',
);
const script = renderer.split('const JS_SCRIPT: &str = r#"')[1].split('"#;')[0];

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

describe('insights HTML report script', () => {
  it('copies the displayed text with its original quotes and punctuation', async () => {
    vi.useFakeTimers();
    document.body.innerHTML = `
      <div class="feature-code">
        <code>&quot;quoted&quot; &amp; &lt;examples&gt; &#39;apostrophes&#39;</code>
        <button class="copy-btn">Copy</button>
      </div>
    `;
    const writeText = vi.fn().mockResolvedValue(undefined);
    const clipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    try {
      window.eval(script);
      document.querySelector<HTMLButtonElement>('button')!.click();
      expect(writeText).toHaveBeenCalledExactlyOnceWith('"quoted" & <examples> \'apostrophes\'');
      await vi.runAllTimersAsync();
    } finally {
      if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard);
      else Reflect.deleteProperty(navigator, 'clipboard');
    }
  });

  it('uses original UTC minutes once and follows the selected timezone', () => {
    document.body.innerHTML = `
      <div id="time-of-day-chart">
        <select id="tz-selector"></select>
        <div id="time-bars"></div>
      </div>
    `;
    const chart = document.getElementById('time-of-day-chart')!;
    chart.dataset.minutes = JSON.stringify({
      [Date.UTC(2026, 10, 19, 10, 0) / 60_000]: 2,
      [Date.UTC(2026, 10, 19, 12, 30) / 60_000]: 3,
      [Date.UTC(2026, 6, 19, 21, 30) / 60_000]: 5,
    });
    chart.dataset.labels = JSON.stringify(['Morning', 'Afternoon', 'Evening', 'Night']);
    window.eval(script);

    const selector = document.querySelector<HTMLSelectElement>('#tz-selector')!;
    const countsIn = (zone: string) => {
      selector.value = zone;
      selector.dispatchEvent(new Event('change'));
      return [...document.querySelectorAll('.bar-value')].map(element => Number(element.textContent));
    };

    expect(countsIn('UTC')).toEqual([2, 3, 5, 0]);
    expect(countsIn('Asia/Shanghai')).toEqual([0, 0, 5, 5]);
    expect(countsIn('Asia/Kolkata')).toEqual([0, 2, 3, 5]);
    expect(countsIn('America/New_York')).toEqual([3, 5, 0, 2]);
  });
});
