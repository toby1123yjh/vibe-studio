import { expect, test } from "@playwright/test";

for (const [executor, name] of [
  ["CLAUDE_CODE", "Claude Code"],
  ["OH_MY_PI", "Oh My Pi"],
] as const) {
  test(`${name} uses native installer without npm registry`, async ({
    page,
  }) => {
    await page.goto(`/?signedOut=true&missing=${executor}`);
    await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await page
      .getByRole("button", { name: "Install agent", exact: true })
      .click();
    await expect(
      page.getByRole("textbox", { name: "npm registry (optional)" }),
    ).toHaveCount(0);
    await page
      .getByRole("button", { name: "Start installation", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Install agent", exact: true }),
    ).toHaveCount(0);
    expect(
      await page.evaluate(() => window.agentInstallProbe.registry),
    ).toBeNull();
  });
}

declare global {
  interface Window {
    agentInstallProbe: {
      starts: number;
      registry: string | null;
      installed: boolean;
    };
    agentInstallJobProbe: {
      startedExecutors: string[];
      polledExecutors: string[];
    };
  }
}

for (const [executor, name] of [
  ["CODEX", "Codex"],
  ["GEMINI", "Gemini"],
  ["OPENCODE", "OpenCode"],
  ["DEEPSEEK_HARNESS", "DeepSeek Harness"],
] as const) {
  test(`${name} npm install forwards registry and rescans availability`, async ({
    page,
  }) => {
    await page.goto(`/?signedOut=true&missing=${executor}`);
    await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await page
      .getByRole("button", { name: "Install agent", exact: true })
      .click();
    await page
      .getByRole("textbox", { name: "npm registry (optional)" })
      .fill("https://registry.npmjs.org");
    await page
      .getByRole("button", { name: "Start installation", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Installing…", exact: true }),
    ).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "Install agent", exact: true }),
    ).toHaveCount(0);
    expect(await page.evaluate(() => window.agentInstallProbe)).toEqual({
      starts: 1,
      registry: "https://registry.npmjs.org",
      installed: true,
    });
  });
}

for (const [executor, name] of [
  ["CODEX", "Codex"],
  ["OPENCODE", "OpenCode"],
  ["DEEPSEEK_HARNESS", "DeepSeek Harness"],
] as const) {
  test(`${name} manual installation supersedes previous failed job`, async ({
    page,
  }) => {
    await page.goto(`/?signedOut=true&missing=${executor}&installFails=true`);
    await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await page
      .getByRole("button", { name: "Install agent", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Start installation", exact: true })
      .click();
    await expect(
      page.getByText("Fixture download failed", { exact: true }),
    ).toBeVisible();
    await page.evaluate(() => {
      window.agentInstallProbe.installed = true;
    });
    await page
      .getByRole("button", { name: "Rescan host", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Install agent", exact: true }),
    ).toHaveCount(0);
    expect(await page.evaluate(() => window.agentInstallProbe.starts)).toBe(1);
  });
}

test("all six providers can be selected without falling back to Codex", async ({
  page,
}) => {
  await page.goto("/?signedOut=true");
  for (const name of [
    "Codex",
    "Claude Code",
    "Gemini",
    "Oh My Pi",
    "OpenCode",
    "DeepSeek Harness",
  ]) {
    const button = page.getByRole("button", { name: new RegExp(`^${name}`) });
    await button.click();
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await expect(
      page.getByRole("heading", { name, exact: true }),
    ).toBeVisible();
  }
  await expect(
    page.getByText("DeepSeek Harness is a developer preview.", {
      exact: false,
    }),
  ).toBeVisible();
});

test("switching missing providers does not reuse another provider installation job", async ({
  page,
}) => {
  await page.goto(
    "/?signedOut=true&missing=OPENCODE,DEEPSEEK_HARNESS&installPending=true",
  );
  await page.getByRole("button", { name: /^OpenCode/ }).click();
  await page
    .getByRole("button", { name: "Install agent", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "npm registry (optional)" })
    .fill("https://registry.npmjs.org");
  await page
    .getByRole("button", { name: "Start installation", exact: true })
    .click();
  const logs = page.locator(".vk-agent-center__install-logs");
  await expect(logs).not.toHaveAttribute("open", "");
  await logs.locator("summary").click();
  await expect(logs).toHaveAttribute("open", "");
  await expect(page.getByLabel("Installation output")).toContainText(
    "OPENCODE",
  );

  await page.getByRole("button", { name: /^DeepSeek Harness/ }).click();
  await expect(page.getByLabel("Installation output")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Install agent", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "npm registry (optional)" }),
  ).toHaveValue("");
  await page
    .getByRole("button", { name: "Start installation", exact: true })
    .click();
  await expect(page.getByLabel("Installation output")).toContainText(
    "DEEPSEEK_HARNESS",
  );
  await expect(page.getByLabel("Installation output")).not.toContainText(
    "OPENCODE",
  );
  expect(
    await page.evaluate(() => window.agentInstallJobProbe.startedExecutors),
  ).toEqual(["OPENCODE", "DEEPSEEK_HARNESS"]);
  await expect
    .poll(() =>
      page.evaluate(() => window.agentInstallJobProbe.polledExecutors),
    )
    .toContain("DEEPSEEK_HARNESS");
});

test("invalid registry cannot launch installer on narrow screens", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto("/?signedOut=true&missing=CODEX");
  await page
    .getByRole("button", { name: "Install agent", exact: true })
    .click();
  await page
    .getByRole("textbox", { name: "npm registry (optional)" })
    .fill("https://user:password@example.com");
  await expect(
    page.getByRole("button", { name: "Start installation", exact: true }),
  ).toBeDisabled();
  expect(await page.evaluate(() => window.agentInstallProbe.starts)).toBe(0);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(375);
});
