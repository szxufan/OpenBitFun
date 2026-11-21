import { validateGenUISchema, type GenUIContractIssue } from './genUIContract';
import type { GenUIAppDefinition, GenUIAppState } from './genUITypes';
export function validateGenUIState(app: GenUIAppDefinition, state: unknown): GenUIContractIssue[] {
  const issues = validateGenUISchema(state, { type: 'object' }, 'app_state');
  if (issues.length)
    return issues;
  if (app.schemaVersion !== 2 || !app.state)
    return [{ path: 'app', code: 'version', severity: 'error', message: 'Unsupported application version; original data retained' }];
  for (const [key, declaration] of Object.entries(app.state))
    if (Object.prototype.hasOwnProperty.call(state as object, key))
      issues.push(...validateGenUISchema((state as GenUIAppState)[key], { type: declaration.type }, 'app_state.' + key));
  return issues;
}
export function genUIContentKey(value: unknown): string {
  return JSON.stringify(value, (_key, entry) => entry && typeof entry === 'object' && !Array.isArray(entry)
    ? Object.fromEntries(Object.keys(entry).sort().map(key => [key, entry[key]])) : entry);
}
/** Recorded values (including null) take precedence over declaration defaults. */
export function recordedGenUIState(app: GenUIAppDefinition, persisted?: GenUIAppState): GenUIAppState {
  const values = { ...persisted };
  for (const [key, declaration] of Object.entries(app.state ?? {})) {
    if (!Object.prototype.hasOwnProperty.call(values, key)) values[key] = structuredClone(declaration.default);
  }
  return values;
}
