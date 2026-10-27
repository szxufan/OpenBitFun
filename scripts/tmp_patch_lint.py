import io, re

p = 'src/flow_chat/components/ModelSelector.tsx'
s = io.open(p, encoding='utf-8').read()
lines = s.split('\n')

# Extract handleRoutingMode block (line 1339 through its closing '};').
# It spans 1339..1367 (1-based, inclusive) per earlier reads; locate robustly.
start = None
for i, line in enumerate(lines):
    if line.strip().startswith('const handleRoutingMode = async'):
        start = i
        break
assert start is not None
end = None
depth = 0
seen_open = False
for i in range(start, len(lines)):
    depth += lines[i].count('{') - lines[i].count('}')
    if '{' in lines[i]:
        seen_open = True
    if seen_open and depth == 0 and lines[i].rstrip().endswith('};'):
        end = i
        break
assert end is not None
routing_block = lines[start:end + 1]

# Extract handleSelectModelFromMenu block.
menu_start = None
for i, line in enumerate(lines):
    if line.strip().startswith('const handleSelectModelFromMenu = async'):
        menu_start = i
        break
assert menu_start is not None
menu_end = None
depth = 0
seen_open = False
for i in range(menu_start, len(lines)):
    depth += lines[i].count('{') - lines[i].count('}')
    if '{' in lines[i]:
        seen_open = True
    if seen_open and depth == 0 and lines[i].rstrip().endswith('};'):
        menu_end = i
        break
assert menu_end is not None
menu_block = lines[menu_start:menu_end + 1]

# Remove both blocks (later indices first).
del lines[menu_start:menu_end + 1]
del lines[start:end + 1]

# Find where handleSelectModel's useCallback ends now: the dep-array line
# ending with ']);' that contains 'targetIsSubagent' followed by ']);'.
insert_at = None
for i, line in enumerate(lines):
    if 'activeSession?.workspaceId,' in line:
        # next line should be '  ]);'
        if lines[i + 1].strip() == ']);'):
            insert_at = i + 2
            break
assert insert_at is not None

# Wrap handleSelectModelFromMenu in useCallback? Lint asked for it because a
# later useCallback depends on it. Wrap it with the minimal dependency set.
wrapped_menu = [
    '  const handleSelectModelFromMenu = useCallback(async (modelId: string) => {',
] + menu_block[0:0] + [
] + [line[2:] if line.startswith('  ') else line for line in []] + []
# Simpler: transform block text.
menu_text = '\n'.join(menu_block)
menu_text = menu_text.replace(
    'const handleSelectModelFromMenu = async (modelId: string) => {',
    'const handleSelectModelFromMenu = useCallback(async (modelId: string) => {',
)
menu_text = menu_text.rstrip()
assert menu_text.endswith('};')
menu_text = menu_text[:-2].rstrip() + '\n  }, [handleRoutingMode, handleSelectModel, hasSettingsPanel,\n    isNativeModelSelection, modelSelectionScopeKey, routing.mode]);'
menu_lines = menu_text.split('\n')

new_block = routing_block + [''] + menu_lines
lines[insert_at:insert_at] = new_block

# Drop the now-unnecessary deps from handleSelectModel's array.
source = '\n'.join(lines)
for dep in ['    currentModelMode,\n', '    currentNativeModelId,\n', '    isNativeModelSelection,\n']:
    # Only the handleSelectModel array contains targetIsSubagent; those dep
    # lines appear in multiple arrays, so scope the removal to the block
    # between 'const handleSelectModel' and 'const handleRoutingMode'.
    m = re.search(r'(const handleSelectModel = useCallback[\s\S]*?targetIsSubagent,\n)', source)
    assert m, 'handleSelectModel block not found'
    block_text = m.group(1)
    block_text = block_text.replace(dep, '')
    source = source[:m.start(1)] + block_text + source[m.end(1):]

io.open(p, 'w', encoding='utf-8', newline='').write(source)
print('reordered and deps cleaned')
