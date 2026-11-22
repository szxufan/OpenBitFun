import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import '@openbitfun/ui/styles.css';
import '@openbitfun/design-tokens/tokens.css';
import '@openbitfun/theme-openbitfun/default.css';
import '../../src/app/styles/index.scss';
import { I18nProvider, i18nService } from '../../src/infrastructure/i18n';
import { OpenBitFunDesignSystemProvider } from '../../src/infrastructure/design-system';
import { appearanceRuntime, buildBuiltinAppearance, openBitFunDarkPalette } from '../../src/infrastructure/appearance';
import { workspaceAPI } from '../../src/infrastructure/api';
import { ResourceFileContext } from '../../src/infrastructure/api/ResourceFileContext';
import { getActiveSurfaceId } from '../../src/infrastructure/peer-device/deviceSurface';
import { useContentResourceStore } from '../../src/app/workbench/contentResourceStore';
import MarkdownEditor from '../../src/tools/editor/components/MarkdownEditor';
import { EditorDocument, EditorDocumentContext } from '../../src/tools/editor/services/EditorDocument';
import { Preview } from './preview-reference';

// Only the filesystem boundary is replaced. The production file editor, parser,
// renderer, dirty tracking, disk polling and save/conflict flow all run normally.
const endpoint = 'http://127.0.0.1:1450';
const filePath = '/workspace/docs/test.md';
const scope = { surfaceId: getActiveSurfaceId(), workspaceId: 'fixture-workspace', workspacePath: '/workspace' };
const editorDocument = new EditorDocument('markdown-browser-fixture', scope, filePath);
workspaceAPI.readWorkspaceFile = async () => (await fetch(`${endpoint}/file`)).text();
workspaceAPI.getWorkspaceFileMetadata = async () => (await fetch(`${endpoint}/metadata`)).json();
workspaceAPI.writeWorkspaceFile = async (_workspace, _file, content) => {
  const response = await fetch(`${endpoint}/file`, { method: 'PUT', body: content });
  if (!response.ok) throw new Error('Fixture file write failed');
};
await i18nService.initialize();
await appearanceRuntime.initialize(buildBuiltinAppearance(openBitFunDarkPalette));

const showReference = new URLSearchParams(location.search).has('reference');
const referenceContent = showReference ? await workspaceAPI.readWorkspaceFile(scope.workspaceId, filePath) : '';

function Fixture() {
  const [dirty, setDirty] = useState(false);
  const [readonly, setReadonly] = useState(false);
  const resources = useContentResourceStore(state => state.resources);
  return <I18nProvider><OpenBitFunDesignSystemProvider>
    <div style={{ height: '100vh', display: 'flex', flexDirection: 'column', maxWidth: 960, margin: 'auto' }}>
      <div><button data-testid="readonly" onClick={() => setReadonly(!readonly)}>Toggle readonly</button>
        <output data-testid="dirty">{dirty ? 'Unsaved' : 'Saved'}</output></div>
      <output data-testid="opened-files" hidden>
        {Object.values(resources).flatMap(resource => resource.target.kind === 'file' ? [resource.target.path] : []).join('\n')}
      </output>
      {showReference ? <Preview value={referenceContent} /> : (
        <EditorDocumentContext.Provider value={editorDocument}>
          <ResourceFileContext.Provider value={editorDocument}>
            <MarkdownEditor filePath={filePath} workspacePath={scope.workspacePath} readOnly={readonly}
              onContentChange={(_content, changed) => setDirty(changed)} />
          </ResourceFileContext.Provider>
        </EditorDocumentContext.Provider>
      )}
    </div>
  </OpenBitFunDesignSystemProvider></I18nProvider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
