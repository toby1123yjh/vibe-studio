import { expect, test } from "@playwright/test";

test("built-in board headings stay English while discussion actions follow the UI language", async ({
  page,
}) => {
  await page.goto("/?surface=board&taskColumns");
  for (const [language, action] of [
    ["zh-Hans", "新建讨论"],
    ["en", "New discussion"],
    ["zh-Hant", "新增討論"],
  ]) {
    await page.evaluate(
      async ({ root, language }) => {
        const { default: i18n } = await import(`${root}/i18n/config.ts`);
        await i18n.changeLanguage(language);
      },
      { root: moduleRoot, language },
    );
    const columns = page.locator(".vk-kanban-columns > .vk-kanban-column");
    await expect(columns.locator("h2")).toHaveText([
      "Discuss",
      "Todo",
      "In Progress",
      "In Review",
      "Done",
    ]);
    await expect(
      columns.first().getByRole("button", { name: action, exact: true }),
    ).toBeVisible();
  }
});

const moduleRoot = `/@fs/${process.cwd().replaceAll("\\", "/")}/packages/web-core/src`;

test("browser mode ignores the previous document language", async ({
  page,
}) => {
  await page.goto("/");
  const language = await page.evaluate(async (root) => {
    Object.defineProperty(navigator, "languages", {
      configurable: true,
      value: ["de-DE"],
    });
    const { default: i18n, updateLanguageFromConfig } = await import(
      `${root}/i18n/config.ts`
    );
    await i18n.changeLanguage("zh-Hant");
    updateLanguageFromConfig("BROWSER");
    return {
      language: i18n.resolvedLanguage,
      html: document.documentElement.lang,
    };
  }, moduleRoot);
  expect(language).toEqual({ language: "en", html: "en" });
});

test("an open project deletion dialog switches copy but preserves the target", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .getByRole("button", { name: "More actions for Project 2", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
  await page.evaluate(async (root) => {
    const { default: i18n } = await import(`${root}/i18n/config.ts`);
    await i18n.changeLanguage("zh-Hans");
  }, moduleRoot);
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Project 2");
  await expect(
    dialog.getByRole("button", { name: "删除", exact: true }),
  ).toBeVisible();
  await expect(dialog).not.toContainText("Delete Project?");
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.locator("html")).not.toHaveAttribute(
    "data-delete-requests",
  );
});

test("browser language variants resolve to real bundles and honor preference order", async ({
  page,
}) => {
  await page.goto("/");
  const result = await page.evaluate(async (root) => {
    const { resolveBrowserLanguage } = await import(
      `${root}/i18n/languages.ts`
    );
    return [
      ["zh-CN"],
      ["zh-SG"],
      ["zh-TW"],
      ["zh-HK"],
      ["zh-MO"],
      ["zh-Hans-TW"],
      ["zh-Hant-CN"],
      ["en-GB"],
      ["de-DE", "zh-TW"],
      [],
    ].map(resolveBrowserLanguage);
  }, moduleRoot);
  expect(result).toEqual([
    "zh-Hans",
    "zh-Hans",
    "zh-Hant",
    "zh-Hant",
    "zh-Hant",
    "zh-Hans",
    "zh-Hant",
    "en",
    "zh-Hant",
    "en",
  ]);
});

test("switching language updates mounted project UI and document language without changing session data", async ({
  page,
}) => {
  await page.route("**/api/sessions/recent?*", (route) =>
    route.fulfill({
      json: {
        success: true,
        data: {
          sessions: [
            {
              id: "session-1",
              workspace_id: "workspace-1",
              execution_id: null,
              title: "用户自己的 Session 标题",
            },
          ],
          next_cursor: null,
        },
      },
    }),
  );
  await page.goto("/?defaultProject");
  for (const [language, heading, htmlLang] of [
    ["ZH_HANS", "默认项目", "zh-Hans"],
    ["EN", "Default project", "en"],
    ["ZH_HANT", "預設專案", "zh-Hant"],
  ]) {
    await page.evaluate(
      async ({ root, language }) => {
        const { updateLanguageFromConfig } = await import(
          `${root}/i18n/config.ts`
        );
        updateLanguageFromConfig(language);
      },
      { root: moduleRoot, language },
    );
    await expect(
      page.getByRole("heading", { name: heading, exact: true }),
    ).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", htmlLang);
    await expect(
      page.getByRole("button", { name: "用户自己的 Session 标题" }),
    ).toBeVisible();
  }
});

test("dates and relative time follow the UI language instead of the browser", async ({
  page,
}) => {
  await page.goto("/");
  const results = await page.evaluate(async (root) => {
    const { default: i18n } = await import(`${root}/i18n/config.ts`);
    const { formatLocalizedDateTime, formatRelativeTime } = await import(
      `${root}/shared/lib/date.ts`
    );
    const output = [];
    for (const language of ["zh-Hans", "en"]) {
      await i18n.changeLanguage(language);
      const value = "2026-09-21T12:00:00Z";
      const options = {
        year: "numeric",
        month: "long",
        day: "numeric",
      } as const;
      output.push({
        actual: formatLocalizedDateTime(value, options),
        expected: new Date(value).toLocaleString(language, options),
        now: formatRelativeTime(new Date().toISOString()),
        future: formatRelativeTime(new Date(Date.now() + 61_000).toISOString()),
        invalid: formatLocalizedDateTime("native-invalid-date"),
      });
    }
    return output;
  }, moduleRoot);
  for (const result of results) {
    expect(result.actual).toBe(result.expected);
    expect(result.invalid).toBe("native-invalid-date");
  }
  expect(results[0].now).toBe("现在");
  expect(results[1].now).toBe("now");
  expect(results[1].future).toBe("in 1 minute");
});
