const { existsSync } = require('node:fs');
const path = require('node:path');

// Both halves of a desktop dev session must advertise the same optional kit.
function visualizeDevGuide(repoRoot) {
  const kit = path.join(repoRoot, 'src/crates/assembly/core/builtin_skills/visualize');
  const guide = path.join(kit, 'authoring/SKILL.md');
  const assets = path.join(kit, 'upstream/skills/visualize/assets');
  return [guide, path.join(assets, 'visualize.html'), path.join(assets, 'standalone-host-bridge.js')]
    .every(file => existsSync(file)) ? guide : undefined;
}

module.exports = { visualizeDevGuide };
