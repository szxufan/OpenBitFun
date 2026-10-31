#!/usr/bin/env node
// Resolve the GitHub Release body for a version, mirroring the ZCode release
// pipeline: a maintained per-version note wins, and a conventional-commit
// digest of the tag range is the fallback so publishing never fails just
// because the note file is missing.
//
// Priority:
//   1. scripts/release-notes/<version>.md (hand-written product summary, also
//      consumed by the updater manifest generator)
//   2. Generated from `git log <prev-tag>..<tag>` grouped by conventional
//      commit type (feat/fix/perf/refactor/docs; ci/chore/test stay hidden)
//   3. Minimal fallback text when neither is available
//
// Usage:
//   node scripts/release-notes/github-release-notes.mjs --version 1.0.9 [--notes-dir scripts/release-notes] [--repo owner/name]
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const usage = `Usage:
  node scripts/release-notes/github-release-notes.mjs --version <semver> [--notes-dir <dir>] [--repo <owner/name>]

Options:
  --version <semver>   Required. Release version, e.g. 1.0.9 (no leading "v").
  --notes-dir <dir>    Directory holding per-version notes. Defaults to scripts/release-notes.
  --repo <owner/name>  Repository for commit links. Defaults to GITHUB_REPOSITORY.
  --help, -h           Show this help.
`;

// Sections mirror release-please-config.json so a generated note uses the same
// vocabulary as the upstream changelog; unlisted types (ci, chore, test, ...)
// stay hidden to keep the release page readable.
const SECTION_BY_TYPE = {
  feat: "Features",
  fix: "Bug Fixes",
  perf: "Performance Improvements",
  refactor: "Code Refactoring",
  docs: "Documentation",
};
const SECTION_ORDER = [
  "Features",
  "Bug Fixes",
  "Performance Improvements",
  "Code Refactoring",
  "Documentation",
];

export function parseArgs(argv) {
  const options = {
    notesDir: "scripts/release-notes",
    repo: process.env.GITHUB_REPOSITORY || undefined,
    help: false,
    version: undefined,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      options.help = true;
      continue;
    }
    const readValue = (name) => {
      if (arg.includes("=")) {
        return arg.slice(arg.indexOf("=") + 1);
      }
      index += 1;
      if (index >= argv.length) {
        throw new Error(`Missing value for ${name}\n${usage}`);
      }
      return argv[index];
    };
    if (arg === "--version" || arg.startsWith("--version=")) {
      options.version = readValue("--version");
      continue;
    }
    if (arg === "--notes-dir" || arg.startsWith("--notes-dir=")) {
      options.notesDir = readValue("--notes-dir");
      continue;
    }
    if (arg === "--repo" || arg.startsWith("--repo=")) {
      options.repo = readValue("--repo");
      continue;
    }
    throw new Error(`Unknown argument: ${arg}\n${usage}`);
  }

  if (options.help) {
    return options;
  }
  if (!options.version || !/^\d+\.\d+\.\d+/.test(options.version)) {
    throw new Error(`Missing or invalid --version: ${options.version}\n${usage}`);
  }
  return options;
}

// Conventional commit subject: type(scope)?: subject. The scope is optional
// and may itself contain slashes; the subject keeps its original casing.
export function parseCommitSubject(subject) {
  const match = subject.match(/^(\w+)(?:\(([^)]+)\))??:\s+(.+)$/);
  if (!match) {
    return null;
  }
  return { type: match[1], scope: match[2], summary: match[3] };
}

function formatEntry({ scope, summary }, shortSha, repo, fullSha) {
  const title = scope ? `**${scope}:** ${summary}` : summary;
  if (!repo || !fullSha) {
    return `* ${title} (${shortSha})`;
  }
  return `* ${title} ([${shortSha}](https://github.com/${repo}/commit/${fullSha}))`;
}

// Semver ordering with prerelease awareness so v1.0.3-beta.1 sorts before
// v1.0.3 and the previous-tag search skips beta channels correctly.
export function compareVersions(a, b) {
  const parse = (version) => {
    const [core, prerelease] = version.split("-");
    const numbers = core.split(".").map(Number);
    return { numbers, prerelease: prerelease ? prerelease.split(".") : [] };
  };
  const left = parse(a);
  const right = parse(b);
  for (let index = 0; index < 3; index += 1) {
    if (left.numbers[index] !== right.numbers[index]) {
      return left.numbers[index] - right.numbers[index];
    }
  }
  if (left.prerelease.length === 0 && right.prerelease.length === 0) {
    return 0;
  }
  if (left.prerelease.length === 0) {
    return 1;
  }
  if (right.prerelease.length === 0) {
    return -1;
  }
  const length = Math.max(left.prerelease.length, right.prerelease.length);
  for (let index = 0; index < length; index += 1) {
    const l = left.prerelease[index];
    const r = right.prerelease[index];
    if (l === r) {
      continue;
    }
    if (l === undefined) {
      return -1;
    }
    if (r === undefined) {
      return 1;
    }
    const ln = /^\d+$/.test(l);
    const rn = /^\d+$/.test(r);
    if (ln && rn) {
      return Number(l) - Number(r);
    }
    if (ln) {
      return -1;
    }
    if (rn) {
      return 1;
    }
    return l < r ? -1 : 1;
  }
  return 0;
}

// Greatest v* tag strictly below the current version; null when the current
// tag is the first one or no lower valid tag exists.
export function selectPreviousTag(tags, version) {
  const candidates = tags.filter((tag) => {
    if (!/^v\d+\.\d+\.\d+/.test(tag)) {
      return false;
    }
    return compareVersions(tag.slice(1), version) < 0;
  });
  if (candidates.length === 0) {
    return null;
  }
  return candidates.reduce((best, tag) =>
    compareVersions(tag.slice(1), best.slice(1)) > 0 ? tag : best,
  );
}

function groupCommits(entries) {
  const sections = new Map();
  for (const { parsed, shortSha, fullSha } of entries) {
    // Non-conventional subjects (merge leftovers, freeform commits) have no
    // type to group by and stay out of the generated note.
    if (!parsed) {
      continue;
    }
    const section = SECTION_BY_TYPE[parsed.type];
    if (!section) {
      continue;
    }
    if (!sections.has(section)) {
      sections.set(section, []);
    }
    sections.get(section).push({ parsed, shortSha, fullSha });
  }
  return sections;
}

export function renderGeneratedNotes(version, previousTag, entries, repo) {
  const lines = [`## OpenBitFun ${version}`, ""];
  const sections = groupCommits(entries);
  let wroteSection = false;
  for (const section of SECTION_ORDER) {
    const grouped = sections.get(section);
    if (!grouped || grouped.length === 0) {
      continue;
    }
    if (wroteSection) {
      lines.push("");
    }
    lines.push(`### ${section}`);
    for (const { parsed, shortSha, fullSha } of grouped) {
      lines.push(formatEntry(parsed, shortSha, repo, fullSha));
    }
    wroteSection = true;
  }
  if (wroteSection && repo && previousTag) {
    lines.push(
      "",
      `**Full Changelog**: https://github.com/${repo}/compare/${previousTag}...v${version}`,
    );
  }
  return lines.join("\n").trim();
}

export function fallbackNotes(version) {
  return [
    `## OpenBitFun ${version}`,
    "",
    "The release notes for this version were not found (no entry in",
    "scripts/release-notes and the tag history is unavailable).",
    "See the commit history for details.",
  ].join("\n");
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

export function collectCommits(currentTag, previousTag) {
  const range = previousTag ? `${previousTag}..${currentTag}` : currentTag;
  const raw = git(["log", range, "--no-merges", "--format=%s%x1f%H"]);
  if (!raw) {
    return [];
  }
  return raw.split("\n").map((line) => {
    const [subject, fullSha] = line.split("\x1f");
    return {
      parsed: parseCommitSubject(subject),
      subject,
      shortSha: fullSha.slice(0, 7),
      fullSha,
    };
  });
}

export function extractSection(markdown, version) {
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(`^##\\s+\\[?v?${escaped}(?![\\d.])`, "m");
  const match = markdown.match(pattern);
  if (!match) {
    return null;
  }
  const start = match.index;
  const nextHeading = markdown.indexOf("\n## ", start + match[0].length);
  const section =
    nextHeading === -1 ? markdown.slice(start) : markdown.slice(start, nextHeading);
  return section.trim();
}

export async function resolveNotes(options) {
  const notePath = path.join(options.notesDir, `${options.version}.md`);
  const noteFile = existsSync(notePath)
    ? (await readFile(notePath, "utf8")).trim()
    : null;
  if (noteFile) {
    return noteFile;
  }

  try {
    const tag = `v${options.version}`;
    git(["rev-parse", "-q", "--verify", `refs/tags/${tag}`]);
    const previousTag = selectPreviousTag(
      git(["tag", "-l", "v*"]).split("\n").filter(Boolean),
      options.version,
    );
    const entries = collectCommits(tag, previousTag);
    const generated = renderGeneratedNotes(options.version, previousTag, entries, options.repo);
    return generated || fallbackNotes(options.version);
  } catch {
    return fallbackNotes(options.version);
  }
}

export async function run(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    return usage;
  }
  return resolveNotes(options);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    console.log(await run());
  } catch (error) {
    process.stderr.write(`${error.message || String(error)}\n`);
    process.exitCode = 1;
  }
}
