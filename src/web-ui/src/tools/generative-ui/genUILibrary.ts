import type { ReactNode } from 'react';
import type { Library } from '@openuidev/react-lang';
import type { GenUILibraryContract } from './genUIContract';
import type { GenUIRenderNode } from './genUITypes';
export interface GenUIComponentProps {
  view: GenUIRenderNode;
  slots: Record<string, ReactNode>;
  /** Render component-valued props such as Text([...]) and Popover content. */
  renderNode: (node: unknown) => ReactNode;
  disabled: boolean;
  emit: (event: string, value?: unknown) => void;
  /** OpenUI renderer state; adapters can defer expensive work while source streams. */
  isStreaming?: boolean;
}
export interface GenUIComponentImplementation {
  render: (props: GenUIComponentProps) => ReactNode;
}
export interface GenUIRendererSource {
  /** Stable source family used for review and revision diagnostics. */
  kind: 'openbitfun-host' | 'openui-react';
  /** Package or repository path that supplies the renderer implementation. */
  source: string;
  /** Upstream/source revision, when the implementation is derived externally. */
  revision?: string;
}
export interface GenUIImplementations {
  components: Record<string, GenUIComponentImplementation>;
  /** Per-component provenance keeps renderer ownership inside the adapter. */
  rendererSources: Record<string, GenUIRendererSource>;
  functions: Record<string, (...args: any[]) => unknown>;
  /** A native OpenUI library used when the profile must retain upstream renderers verbatim. */
  nativeLibrary?: Library;
}
export interface GenUILibrary extends GenUIImplementations {
  contract: GenUILibraryContract;
  nativeLibrary?: Library;
}
/** Assembly binds declarations to trusted implementations; incomplete libraries cannot run. */
export function createGenUILibrary(contract: GenUILibraryContract, implementations: GenUIImplementations): GenUILibrary {
  const bind = <T,>(declared: Record<string, unknown>, available: Record<string, T>, kind: string) => Object.fromEntries(Object.keys(declared).map(name => {
    if (!Object.prototype.hasOwnProperty.call(available, name))
      throw new Error(contract.id + ': missing ' + kind + ' implementation: ' + name);
    return [name, available[name]];
  }));
  if (implementations.nativeLibrary) {
    for (const name of Object.keys(contract.components)) {
      if (!Object.prototype.hasOwnProperty.call(implementations.nativeLibrary.components, name))
        throw new Error(contract.id + ': native OpenUI library is missing component implementation: ' + name);
    }
    const nativeFunctions = implementations.nativeLibrary.functions ?? {};
    for (const name of Object.keys(contract.functions)) {
      if (!Object.prototype.hasOwnProperty.call(nativeFunctions, name))
        throw new Error(contract.id + ': native OpenUI library is missing function implementation: ' + name);
    }
    return { contract, components: {}, rendererSources: {}, functions: {}, nativeLibrary: implementations.nativeLibrary };
  }
  for (const name of Object.keys(contract.components)) {
    const component = implementations.components[name];
    if (!component || typeof component.render !== 'function')
      throw new Error(contract.id + ': missing component implementation: ' + name);
    const source = implementations.rendererSources[name];
    if (!source || !source.kind || !source.source)
      throw new Error(contract.id + ': missing renderer provenance: ' + name);
  }
  for (const name of Object.keys(contract.functions)) {
    if (typeof implementations.functions[name] !== 'function')
      throw new Error(contract.id + ': missing function implementation: ' + name);
  }
  return {
    contract,
    components: bind(contract.components, implementations.components, 'component'),
    rendererSources: bind(contract.components, implementations.rendererSources, 'renderer provenance'),
    functions: bind(contract.functions, implementations.functions, 'function'),
    ...(implementations.nativeLibrary ? { nativeLibrary: implementations.nativeLibrary } : {}),
  };
}
