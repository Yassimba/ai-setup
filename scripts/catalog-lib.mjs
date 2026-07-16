import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

import { parse } from "yaml";

function parseFrontmatter(content, path) {
  const match = content.match(/^---\s*\n([\s\S]*?)\n---(?:\s*\n|$)/);
  if (!match) throw new Error(`missing frontmatter: ${path}`);
  const value = parse(match[1]);
  if (!value || typeof value !== "object") throw new Error(`invalid frontmatter: ${path}`);
  return value;
}

function readSkillEntries(manifest) {
  if (!Array.isArray(manifest?.groupings)) {
    throw new Error("skills.sh.json must contain a groupings array");
  }
  const entries = manifest.groupings.flatMap((group) =>
    Array.isArray(group?.skills) ? group.skills.map((name) => ({ name, group: group.title })) : [],
  );
  if (
    !entries.every(
      ({ name, group }) =>
        typeof name === "string" &&
        name.length > 0 &&
        typeof group === "string" &&
        group.length > 0,
    )
  ) {
    throw new Error("every reviewed skill and grouping title must be a non-empty string");
  }
  const names = entries.map(({ name }) => name);
  const duplicates = names.filter((name, index) => names.indexOf(name) !== index);
  if (duplicates.length > 0) {
    throw new Error(`duplicate reviewed skill names: ${[...new Set(duplicates)].join(", ")}`);
  }
  return entries;
}

async function indexSkillDirectories(repoRoot) {
  const index = new Map();
  const skillsRoot = join(repoRoot, "skills");
  for (const skillEntry of await readdir(skillsRoot, { withFileTypes: true })) {
    if (!skillEntry.isDirectory()) continue;
    const skillPath = join(skillsRoot, skillEntry.name, "SKILL.md");
    try {
      const frontmatter = parseFrontmatter(await readFile(skillPath, "utf8"), skillPath);
      const dependencies = await readSkillDependencies(join(skillsRoot, skillEntry.name));
      index.set(skillEntry.name, { frontmatter, path: skillPath, dependencies });
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  return index;
}

// skills/<name>/deps.yml — the skills this skill invokes; installers pull
// them in transitively. Validated against the reviewed set by the caller.
async function readSkillDependencies(skillRoot) {
  const depsPath = join(skillRoot, "deps.yml");
  let raw;
  try {
    raw = await readFile(depsPath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
  const value = parse(raw);
  const skills = value?.skills;
  if (
    !Array.isArray(skills) ||
    skills.length === 0 ||
    !skills.every((name) => typeof name === "string" && name.length > 0)
  ) {
    throw new Error(`deps.yml must hold a non-empty skills list of names: ${depsPath}`);
  }
  return skills;
}

export async function readReviewedSkillCatalog(repoRoot) {
  const manifest = JSON.parse(await readFile(join(repoRoot, "skills.sh.json"), "utf8"));
  const entries = readSkillEntries(manifest);
  const index = await indexSkillDirectories(repoRoot);

  const reviewed = new Set(entries.map(({ name }) => name));
  rejectDependencyCycles(entries, index);
  return entries.map(({ name, group }) => {
    const candidate = index.get(name);
    if (!candidate) throw new Error(`reviewed skill not found: ${name}`);
    if (candidate.frontmatter.name !== name) {
      throw new Error(`skill directory and frontmatter name differ: ${name}`);
    }
    if (typeof candidate.frontmatter.description !== "string") {
      throw new Error(`skill description is missing: ${name}`);
    }
    for (const dependency of candidate.dependencies) {
      if (!reviewed.has(dependency)) {
        throw new Error(`skill ${name} depends on unreviewed skill: ${dependency}`);
      }
    }
    return {
      id: `skill:${name}`,
      kind: "skill",
      group,
      label: name,
      description: candidate.frontmatter.description,
      installTarget: name,
      nextAction: `Ask your coding agent to use the ${name} skill.`,
      dependencies: candidate.dependencies,
    };
  });
}

// The skill dep graph must stay acyclic (AGENTS.md); enforcing it here puts
// the check on the `npm run check` path instead of only the manually-run
// `sync-skills.sh deps`. DFS with a path stack names the cycle it finds.
function rejectDependencyCycles(entries, index) {
  const visited = new Set();
  const stack = [];
  const visit = (name) => {
    const position = stack.indexOf(name);
    if (position !== -1) {
      throw new Error(`skill dependency cycle: ${[...stack.slice(position), name].join(" -> ")}`);
    }
    if (visited.has(name)) return;
    visited.add(name);
    stack.push(name);
    for (const dependency of index.get(name)?.dependencies ?? []) visit(dependency);
    stack.pop();
  };
  for (const { name } of entries) visit(name);
}

export async function readPiPackageCatalog(repoRoot) {
  const packagesRoot = join(repoRoot, "plugins");
  const resources = [];
  for (const entry of await readdir(packagesRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifestPath = join(packagesRoot, entry.name, "package.json");
    let raw;
    try {
      raw = await readFile(manifestPath, "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    const manifest = JSON.parse(raw);
    const catalog = manifest?.aiSetup?.catalog;
    if (!catalog) continue;
    if (manifest.private) throw new Error(`setup Pi package is private: ${manifestPath}`);
    if (
      typeof catalog.label !== "string" ||
      typeof catalog.description !== "string" ||
      typeof manifest.name !== "string"
    ) {
      throw new Error(`invalid aiSetup.catalog metadata: ${manifestPath}`);
    }
    resources.push({
      id: `pi-package:${manifest.name}`,
      kind: "pi-package",
      group: "Pi packages",
      label: catalog.label,
      description: catalog.description,
      installTarget: manifest.name,
      nextAction: catalog.nextAction ?? "Start Pi and use the installed package.",
    });
  }
  return resources.sort((left, right) => left.label.localeCompare(right.label));
}

function parseTomlString(content, key, path) {
  const header = content.split(/^\s*\[/m, 1)[0];
  const match = header.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, "m"));
  if (!match) throw new Error(`missing ${key} in ${path}`);
  return match[1];
}

export async function readHerdrPluginCatalog(repoRoot) {
  const pluginsRoot = join(repoRoot, "plugins");
  const resources = [];
  for (const entry of await readdir(pluginsRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifestPath = join(pluginsRoot, entry.name, "herdr-plugin.toml");
    let content;
    try {
      content = await readFile(manifestPath, "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    const id = parseTomlString(content, "id", manifestPath);
    resources.push({
      id: `herdr-plugin:${id}`,
      kind: "herdr-plugin",
      group: "Herdr plugins",
      label: parseTomlString(content, "name", manifestPath),
      description: parseTomlString(content, "description", manifestPath),
      installTarget: `Yassimba/ai-setup/plugins/${entry.name}`,
      nextAction: "Run `herdr plugin list` to see the installed plugin.",
    });
  }
  return resources.sort((left, right) => left.label.localeCompare(right.label));
}

export async function buildSetupCatalog(repoRoot) {
  const [piPackages, skills, herdrPlugins] = await Promise.all([
    readPiPackageCatalog(repoRoot),
    readReviewedSkillCatalog(repoRoot),
    readHerdrPluginCatalog(repoRoot),
  ]);
  return [...piPackages, ...skills, ...herdrPlugins];
}
