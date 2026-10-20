import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const namespaces = ["common", "projects", "tasks", "settings", "organization"];
const chineseLocales = ["zh-Hans", "zh-Hant"];

test("model aliases and priority explanations are translated with matching parameters", () => {
  const english = catalog("en", "common");
  const keys = [
    "modelSelector.aliasModel",
    "modelSelector.unresolvedAlias",
    "modelSelector.nativeModelId",
    "modelSelector.configuredCatalog",
    "modelSelector.enterModelId",
    "modelSelector.modelId",
    "modelSelector.manualModelHint",
    "modelSelector.modelIdRequired",
    "modelSelector.useModel",
    "kanban.priorityUrgent",
    "kanban.priorityHigh",
    "kanban.priorityMedium",
    "kanban.priorityLow",
  ];
  for (const locale of ["en", "zh-Hans", "zh-Hant", "fr", "ja", "es", "ko"]) {
    const translated = catalog(locale, "common");
    for (const key of keys) {
      const message = translated[key];
      assert.ok(typeof message === "string" && message.trim(), `${locale}/${key}`);
      assert.deepEqual(
        (message.match(/{{.*?}}/g) ?? []).sort(),
        (english[key].match(/{{.*?}}/g) ?? []).sort(),
        `${locale}/${key}`,
      );
      if (chineseLocales.includes(locale)) {
        assert.notEqual(message, english[key], `${locale}/${key}`);
      }
    }
  }
});

test("Discuss column title stays English in every supported locale", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant", "fr", "ja", "es", "ko"]) {
    assert.equal(catalog(locale, "common")["projectSessions.column"], "Discuss");
  }
});

function flatten(object, prefix = "", result = {}) {
  for (const [key, value] of Object.entries(object)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "object" && value !== null) {
      flatten(value, path, result);
    } else {
      assert.equal(typeof value, "string", path);
      result[path] = value;
    }
  }
  return result;
}

function catalog(locale, namespace) {
  return flatten(
    JSON.parse(
      readFileSync(
        new URL(
          `../../packages/web-core/src/i18n/locales/${locale}/${namespace}.json`,
          import.meta.url,
        ),
        "utf8",
      ),
    ),
  );
}

// Product names, file names, protocol names and literal examples are intentional.
// Ordinary interface copy must not be added here merely to silence the test.
const sharedLiterals = new Set([
  'Discuss',
  "Arena",
  "Arena · {{prompt}}",
  "Diff",
  "Git (X ...)",
  "MCP",
  "{{fileId}} · {{format}} · {{scope}}",
  '{\n  "NAME": "value"\n}',
  "--flag=value",
  "Claude Markdown",
  "Gemini TOML",
  "SKILL.md",
  "PR #{{number}}",
  "my-project",
  "https://github.com/owner/repo/pull/123",
  "brew.sh",
  "Git",
  "{{prefix}}/1a2b-task-name",
  "1a2b-task-name",
  "/path/to/your/existing/repo",
  "colleague@example.com",
]);

for (const namespace of namespaces) {
  const english = catalog("en", namespace);
  for (const locale of chineseLocales) {
    const translated = catalog(locale, namespace);
    test(`${locale}/${namespace}: keys and interpolation match English`, () => {
      assert.deepEqual(
        Object.keys(translated).sort(),
        Object.keys(english).sort(),
      );
      for (const [key, value] of Object.entries(english)) {
        const parameters = (text) => (text.match(/{{.*?}}/g) ?? []).sort();
        assert.deepEqual(parameters(translated[key]), parameters(value), key);
      }
    });

    test(`${locale}/${namespace}: no English fallback copy or ambiguous agent/workspace labels`, () => {
      for (const [key, value] of Object.entries(translated)) {
        if (/[a-zA-Z]{3}/.test(value) && value === english[key]) {
          assert.ok(sharedLiterals.has(value), `${key}: ${value}`);
        }
        assert.doesNotMatch(value, /工作区|工作區|智慧體/, key);
        // “代理” remains valid for a network proxy, but not an agent label.
        if (["arena.workspace.agent", "kanban.agent"].includes(key)) {
          assert.equal(value, locale === "zh-Hans" ? "智能体" : "智能體");
        }
      }
    });
  }
}
