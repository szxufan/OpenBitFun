import { zodForSchema } from './genUIOpenUISchema';
import './disableOpenUIDevtools';
import { actions } from './components/openui/actions';
import type { OpenUIError, ParseResult } from '@openuidev/lang-core';
import { mergeGenUIRenderDiagnostics, type GenUIRenderDiagnostic } from './genUIRenderDiagnostics';
import { action, createLibrary, defineComponent, defineFunction, reactive, Renderer, useIsStreaming, useStateField } from '@openuidev/react-lang';
import { z } from 'zod/v4';
import { Fragment, useCallback, useEffect, useMemo, useRef, type ReactNode } from 'react';
import type { GenUILibrary } from './genUILibrary';
import type { GenUIAppState, GenUIOpenUIDocumentSource } from './genUITypes';
import type { GenUIRenderNode } from './genUITypes';
/**
 * OpenUI uses `$name` as the binding identity while GenUI persistence keeps
 * declaration keys as ordinary JSON fields (`name`). Keep that translation at
 * this adapter boundary so the renderer and the durable owner do not each
 * learn the other's state shape.
 */
function toOpenUIState(state?: GenUIAppState): GenUIAppState | undefined {
  if (!state) return state;
  const normalized = { ...state };
  for (const [key, value] of Object.entries(state)) {
    if (!key.startsWith('$') && !Object.prototype.hasOwnProperty.call(normalized, `$${key}`)) normalized[`$${key}`] = value;
  }
  return normalized;
}

/**
 * Adapt the selected product library to OpenUI's language runtime. The
 * catalog remains the source of truth for names, required fields and slot
 * relationships; renderer implementations stay in the trusted product
 * library. OpenUI owns parsing, streaming and state-field semantics.
 */
function createGenUIOpenUILibrary(library: GenUILibrary) {
  const components = Object.entries(library.contract.components).map(([name, contract]) => {
    const props: Record<string, z.ZodTypeAny> = {};
    // OpenUI uses declaration order for positional arguments. Put child slots
    // first so the natural `Col([Text(...)])` form remains composable; the
    // generated prompt exposes the resulting signature to the model.
    for (const slot of Object.keys(contract.slots)) {
      const slotSchema = z.array(z.any());
      props[slot] = contract.slotRequired?.[slot] ? slotSchema : slotSchema.optional();
    }
    const declaredProps = Object.keys(contract.props.properties ?? {});
    const required = new Set((contract.props.required as string[] | undefined) ?? []);
    const declaredSchemas = (contract.props.properties ?? {}) as Record<string, Record<string, unknown>>;
    // Keep required visual props before the optional action slot so a normal
    // Button("Save", @ToAssistant("...")) remains positional and readable.
    for (const key of declaredProps.filter(key => required.has(key))) {
      if (!Object.prototype.hasOwnProperty.call(props, key)) props[key] = zodForSchema(declaredSchemas[key]);
    }
    // OpenUI action is a language-level prop. It is deliberately added only
    // to product components that declare an interaction event, keeping the
    // catalog's trusted visual props unchanged while allowing the selected
    // owner to receive a structured action plan. Its position is part of the
    // generated positional signature and is never reinterpreted at runtime.
    if (contract.action) props.action = action().optional();
    for (const key of declaredProps.filter(key => !required.has(key))) {
      if (!Object.prototype.hasOwnProperty.call(props, key)) props[key] = zodForSchema(declaredSchemas[key]).optional();
    }
    const bindingProps = new Set(contract.bindings ?? []);
    // Preserve OpenUI `$variable` bindings for writable form values. The
    // product catalog remains the source of truth for the prop shape; the
    // binding list opts fields such as value, checked, selected and seconds
    // into OpenUI's StateField semantics.
    const schema = z.object(Object.fromEntries(Object.entries(props).map(([key, value]) => [key, bindingProps.has(key) ? reactive(value) : value])));
    const implementation = library.components[name];
    const component = defineComponent({
      name,
      description: contract.description,
      props: schema,
      component: ({ props: values, renderNode, statementId }) => {
        // react-lang invokes this callback as a component, but its public
        // defineComponent type intentionally keeps the callback lowercase.
        // These hooks therefore follow the renderer's stable callback order.
        // Keep product adapters on the same streaming boundary as OpenUI.
        // Map uses this to wait for a complete marker list before routing.
        // eslint-disable-next-line react-hooks/rules-of-hooks
        const isStreaming = useIsStreaming();
        const rawValues = values as Record<string, unknown>;
        const normalizedValues = { ...rawValues };
        for (const [key, schema] of Object.entries(declaredSchemas)) {
          if (schema.type === 'boolean' && (rawValues[key] === 0 || rawValues[key] === 1)) {
            normalizedValues[key] = rawValues[key] === 1;
          }
        }
        const resolvedValues = { ...normalizedValues };
        for (const key of bindingProps) {
          const raw = normalizedValues[key];
          // `bindingProps` is fixed by the catalog for this component; the
          // renderer invokes one state hook per declared binding in order.
          // eslint-disable-next-line react-hooks/rules-of-hooks
          const field = useStateField(key, raw);
          if (field.isReactive) resolvedValues[key] = field.value;
        }
        const view = {
          key: statementId ?? name,
          node: { id: statementId ?? name, component: name, props: resolvedValues, slots: {}, events: {} },
          props: resolvedValues,
          item: undefined,
          slots: {},
        } as unknown as GenUIRenderNode;
        const slots = Object.fromEntries(Object.keys(contract.slots).map(slot => [
          slot,
          (Array.isArray(resolvedValues[slot])
            ? (resolvedValues[slot] as unknown[]).map((child, index) => <Fragment key={`${statementId ?? name}-${slot}-${index}`}>{renderNode(child)}</Fragment>)
            : []) as ReactNode[],
        ]));
        return implementation.render({
          view,
          slots,
          renderNode,
          // `readOnly` is a host policy, while `isStreaming` belongs to the
          // OpenUI renderer. Keep them separate: OpenUI still mounts the
          // completed tree while a source is streaming, and its trusted
          // components decide which controls are temporarily unavailable.
          disabled: true,
          isStreaming,
          emit: () => {},
        });
      },
    });
    return component;
  });
  // OpenUI Lang already owns the core data calls (Count through Each). They
  // are still part of the product contract and prompt, but registering them
  // again would collide with the runtime's built-ins. Only the extension
  // functions are added to the selected library.
  const openUIBuiltins = new Set(['Count', 'First', 'Last', 'Sum', 'Avg', 'Min', 'Max', 'Sort', 'Filter', 'Round', 'Abs', 'Floor', 'Ceil', 'Each']);
  const functions = Object.entries(library.contract.functions).filter(([name]) => !openUIBuiltins.has(name)).map(([name, contract]) => {
    const optionalParams = new Set(contract.optionalParams ?? []);
    const params = Object.fromEntries(contract.params.map((schema, index) => {
      const field = zodForSchema(schema);
      return [`arg${index}`, optionalParams.has(index) ? field.optional() : field];
    }));
    const implementation = library.functions[name];
    return defineFunction({
      name,
      description: contract.description,
      params: z.object(params),
      returns: zodForSchema(contract.returns),
      fn: args => implementation(...Object.values(args)),
    });
  });
  const root = library.contract.metadata?.rootComponent ?? components[0]?.name;
  if (!root) throw new Error(`${library.contract.id}: OpenUI library has no components`);
  return createLibrary({ components, functions, actions, root });
}

export default function GenUIOpenUIDocument({ source, library, initialState, isStreaming = false, onError }: {
  source: GenUIOpenUIDocumentSource;
  library: GenUILibrary;
  initialState?: GenUIAppState;
  isStreaming?: boolean;
  onError?: (errors: GenUIRenderDiagnostic[]) => void;
}) {
  const openUILibrary = useMemo(() => library.nativeLibrary ?? createGenUIOpenUILibrary(library), [library]);
  const openUIInitialState = useMemo(() => toOpenUIState(initialState), [initialState]);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  // Errors and parse metadata arrive in separate effects. Coalesce both before
  // publishing, and discard pending evidence when the source/library changes.
  const diagnosticFrame = useMemo(() => ({
    sourceContent: source.content, library: openUILibrary, isStreaming,
    errors: [] as OpenUIError[], meta: undefined as ParseResult['meta'] | undefined,
    parsed: false, pending: false, lastKey: undefined as string | undefined,
  }), [source.content, openUILibrary, isStreaming]);
  const currentDiagnosticFrame = useRef<typeof diagnosticFrame | undefined>(diagnosticFrame);
  currentDiagnosticFrame.current = diagnosticFrame;
  useEffect(() => {
    currentDiagnosticFrame.current = diagnosticFrame;
    return () => {
      if (currentDiagnosticFrame.current === diagnosticFrame) currentDiagnosticFrame.current = undefined;
    };
  }, [diagnosticFrame]);
  const publishDiagnostics = useCallback(() => {
    if (diagnosticFrame.pending) return;
    diagnosticFrame.pending = true;
    queueMicrotask(() => {
      diagnosticFrame.pending = false;
      if (currentDiagnosticFrame.current !== diagnosticFrame || (!diagnosticFrame.isStreaming && !diagnosticFrame.parsed)) return;
      const merged = diagnosticFrame.isStreaming ? [] : mergeGenUIRenderDiagnostics(diagnosticFrame.meta, diagnosticFrame.errors);
      const key = JSON.stringify(merged);
      if (key === diagnosticFrame.lastKey) return;
      diagnosticFrame.lastKey = key;
      onErrorRef.current?.(merged);
    });
  }, [diagnosticFrame]);
  const handleError = useCallback((errors: OpenUIError[]) => {
    diagnosticFrame.errors = errors;
    publishDiagnostics();
  }, [diagnosticFrame, publishDiagnostics]);
  const handleParseResult = useCallback((result: ParseResult | null) => {
    diagnosticFrame.meta = result?.meta;
    diagnosticFrame.parsed = true;
    publishDiagnostics();
  }, [diagnosticFrame, publishDiagnostics]);
  return (
    <Renderer
      response={source.content}
      library={openUILibrary}
      isStreaming={isStreaming}
      initialState={openUIInitialState}
      onParseResult={handleParseResult}
      onError={handleError}
    />
  );
}

