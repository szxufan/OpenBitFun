// ECharts resolves its palette into paint values and does not inherit document CSS.
(() => {
  const library = window.echarts;
  const nativeInit = library.init;
  const charts = new Set();
  function readTheme() {
    const style = getComputedStyle(document.documentElement);
    const pixel = document.createElement('canvas');
    pixel.width = pixel.height = 1;
    const paint = pixel.getContext('2d', { willReadFrequently: true });
    const probe = document.createElement('span');
    document.documentElement.append(probe);
    // The canvas and SVG renderers share zrender's sRGB parser. Resolve modern
    // skin colors (including light-dark and color-mix) in the actual guest.
    const read = name => {
      probe.style.color = 'var(' + name + ')';
      const color = getComputedStyle(probe).color;
      if (/^rgba?\(/.test(color)) return color;
      paint.clearRect(0, 0, 1, 1);
      paint.fillStyle = color;
      paint.fillRect(0, 0, 1, 1);
      const [r, g, b, a] = paint.getImageData(0, 0, 1, 1).data;
      return 'rgba(' + [r, g, b, a / 255].join(',') + ')';
    };
    const ink = read('--foreground');
    const label = read('--muted-foreground');
    const border = read('--border');
    const accent = read('--viz-series-1');
    const paper = read('--background');
    const surface = read('--card');
    const axis = {
      axisLabel: { color: label }, nameTextStyle: { color: label },
      axisLine: { lineStyle: { color: border } }, splitLine: { lineStyle: { color: border } },
    };
    const result = {
      darkMode: style.colorScheme === 'dark',
      color: Array.from({ length: 12 }, (_, i) => read('--viz-series-' + (i + 1))),
      backgroundColor: 'transparent',
      textStyle: { color: ink, fontFamily: style.getPropertyValue('--font-sans').trim() },
      title: { textStyle: { color: ink }, subtextStyle: { color: label } },
      legend: { textStyle: { color: ink }, inactiveColor: label, inactiveBorderColor: border },
      visualMap: { textStyle: { color: label }, inRange: { color: [surface, accent] } },
      dataZoom: {
        textStyle: { color: label }, borderColor: border, backgroundColor: surface,
        fillerColor: read('--accent'), handleStyle: { color: paper, borderColor: accent },
      },
      toolbox: { iconStyle: { borderColor: label } },
      tooltip: { backgroundColor: read('--popover'), borderColor: border, textStyle: { color: read('--popover-foreground') } },
      categoryAxis: axis, valueAxis: axis, timeAxis: axis, logAxis: axis,
    };
    probe.remove();
    return result;
  }
  let theme = readTheme();
  let signature = JSON.stringify(theme);
  library.init = function (dom, explicitTheme, options) {
    const existing = library.getInstanceByDom(dom);
    const chart = nativeInit(dom, explicitTheme ?? theme, options);
    if (!existing && explicitTheme == null) {
      charts.add(chart);
      const dispose = chart.dispose.bind(chart);
      chart.dispose = function () { charts.delete(chart); return dispose(); };
    }
    return chart;
  };

  // setTheme recreates ECharts' original options, losing later setOption data,
  // legend selection and zoom. Merge just the paint values still owned by the host.
  function patchDefaults(current, old, next) {
    if (Array.isArray(old)) {
      if (Array.isArray(current) && old.length === current.length && old.every((v, i) => v === current[i])) return next;
    } else if (old && typeof old === 'object') {
      const patch = {};
      for (const key of Object.keys(old)) {
        const value = patchDefaults(current?.[key], old[key], next[key]);
        if (value !== undefined) patch[key] = value;
      }
      if (Object.keys(patch).length) return patch;
    } else if (current === old && old !== next) return next;
  }
  function update(chart, previous) {
    const current = chart.getOption();
    if (!current) return;
    const patch = {};
    for (const key of ['color', 'darkMode', 'textStyle']) {
      const value = patchDefaults(current[key], previous[key], theme[key]);
      if (value !== undefined) patch[key] = value;
    }
    for (const key of ['title', 'legend', 'tooltip', 'visualMap', 'dataZoom', 'toolbox', 'xAxis', 'yAxis', 'radiusAxis', 'angleAxis']) {
      if (!current[key]?.length) continue;
      const components = current[key].map(component => {
        const role = key.endsWith('Axis') ? component.type + 'Axis' : key;
        return patchDefaults(component, previous[role], theme[role]) ?? {};
      });
      if (components.some(component => Object.keys(component).length)) patch[key] = components;
    }
    if (Object.keys(patch).length) chart.setOption(patch, { silent: true });
  }
  addEventListener('openbitfun:figure-appearance', () => {
    const next = readTheme();
    const key = JSON.stringify(next);
    if (signature === key) return;
    const previous = theme;
    theme = next;
    signature = key;
    for (const chart of charts) update(chart, previous);
  });
})();
