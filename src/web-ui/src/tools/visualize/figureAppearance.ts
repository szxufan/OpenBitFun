import guestTypography from './figureTypography.json';
import { cssVariables } from '@openbitfun/design-tokens';
import { themeCssVariables } from '@openbitfun/theme-openbitfun';

// Map the kit's presentation vocabulary to the colors at this figure's position in FlowChat.
// Only resolved presentation values cross into the guest. Skin stylesheets and artwork stay in the host.
const colors = {
  '--background': 'color.surface.canvas',
  '--foreground': 'color.content.primary',
  '--card': 'color.surface.panel',
  '--card-foreground': 'color.content.primary',
  '--popover': 'color.surface.raised',
  '--popover-foreground': 'color.content.primary',
  '--primary': 'color.content.primary',
  '--primary-foreground': 'color.content.inverse',
  '--secondary': 'color.action.secondary.background',
  '--secondary-foreground': 'color.action.secondary.content',
  '--muted-foreground': 'color.content.secondary',
  '--accent': 'color.accent.surface',
  '--accent-foreground': 'color.accent.default',
  '--destructive': 'color.status.danger.content',
  '--border': 'color.border.default',
  '--input': 'color.border.strong',
  '--ring': 'color.focus.ring',
  '--orange': 'color.identity.project.orange',
  '--yellow': 'color.identity.project.yellow',
  '--purple': 'color.accent.secondary',
  '--blue': 'color.status.info.content',
  '--red': 'color.status.danger.content',
  '--green': 'color.status.success.content',
  '--viz-series-1': 'color.accent.default',
  '--viz-series-2': 'color.status.warning.content',
  '--viz-series-3': 'color.status.success.content',
  '--viz-series-4': 'color.status.danger.content',
  '--viz-series-5': 'color.accent.secondary',
  '--viz-series-6': 'color.status.info.content',
  '--viz-series-7': 'color.identity.project.cyan',
  '--viz-series-8': 'color.identity.project.pink',
  '--viz-series-9': 'color.identity.project.yellow',
  '--viz-series-10': 'color.identity.project.purple',
  '--viz-series-11': 'color.identity.project.green',
  '--viz-series-12': 'color.identity.project.orange',
} as const satisfies Record<string, keyof typeof themeCssVariables>;

// Both the read-only HTML viewer and the live kit consume these guest font aliases.
const typographySources: Record<string, { property: string; variable: string }> = {
  body: { property: 'font-family', variable: cssVariables['type.flow.body.fontFamily'] },
  code: { property: 'font-family', variable: cssVariables['type.flow.code.fontFamily'] },
  size: { property: 'font-size', variable: cssVariables['type.flow.body.fontSize'] },
};
const typography = Object.entries(guestTypography).map(([name, role]) => ({ name, ...typographySources[role] }));
export const staticFigureTypography = '@layer visualize-base {:root{' +
  typography.map(({ name, variable }) => name + ':var(' + variable + ')').join(';') + '}}';

const documentTypography = [
  'type.flow.body.lineHeight', 'type.heading.panel.fontSize', 'type.heading.panel.lineHeight',
  'type.heading.dialog.fontSize', 'type.heading.dialog.lineHeight',
  'type.heading.section.fontSize', 'type.heading.section.lineHeight',
  'type.heading.section.fontWeight', 'type.body.sm.fontSize',
] as const;

export function readFigureStyle(host: HTMLElement, fallbackScheme?: string): Record<string, string> {
  const style = getComputedStyle(host);
  const scheme = style.colorScheme.trim();
  const mode = scheme === 'dark' || scheme === 'light'
    ? scheme
    : document.documentElement.dataset.openbitfunAppearanceMode ?? fallbackScheme;
  const resolved: Record<string, string> = { 'color-scheme': mode === 'dark' ? 'dark' : 'light' };
  for (const [name, token] of Object.entries(colors)) {
    const value = style.getPropertyValue(themeCssVariables[token]).trim();
    if (value && CSS.supports('color', value)) {
      resolved[name] = value;
      resolved[themeCssVariables[token]] = value;
    }
  }
  for (const token of ['color.action.primary.background', 'color.action.primary.content', 'color.action.primary.hover'] as const) {
    const name = themeCssVariables[token];
    const value = style.getPropertyValue(name).trim();
    if (value && CSS.supports('color', value)) resolved[name] = value;
  }
  for (const { name, property, variable } of typography) {
    const value = style.getPropertyValue(variable).trim();
    if (value && CSS.supports(property, value)) {
      resolved[name] = value;
      resolved[variable] = value;
    }
  }
  for (const token of documentTypography) {
    const name = cssVariables[token];
    const value = style.getPropertyValue(name).trim();
    if (value) resolved[name] = value;
  }
  // Figures use muted for secondary marks as well as fills. A subtle host overlay
  // (3% on the light skin) disappears against the plotting surface.
  resolved['--muted'] = 'color-mix(in srgb, var(--openbitfun-color-content-primary) 50%, var(--openbitfun-color-surface-canvas))';
  return resolved;
}
