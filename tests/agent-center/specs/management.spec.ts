import { expect, test, type Page } from "@playwright/test";
import type {
  AgentSettingsProvider,
  CreateAgentCommandRequest,
  CreateAgentToolRequest,
} from "../../../shared/types";

declare global {
  interface Window {
    agentManagementProbe: {
      createdTools: CreateAgentToolRequest[];
      createdCommands: CreateAgentCommandRequest[];
      discoveredSettings: AgentSettingsProvider[];
      savedProfiles: string[];
    };
  }
}

const openDialog = (page: Page) =>
  page.locator('.vk-keyboard-dialog[data-state="open"]');

for (const [name, provider, model] of [
  ["OpenCode", "opencode", "FixtureProvider/FixtureModel"],
  ["DeepSeek Harness", "deepseek_harness", "FixtureDshModel"],
] as const) {
  test(`${name} loads native settings into the real configuration panel`, async ({
    page,
  }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/?signedOut=true");
    await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await expect(page.getByText(model, { exact: true })).toBeVisible();
    await page
      .getByRole("button", { name: "Configuration Profiles", exact: true })
      .click();
    const panel = page.getByTestId("agent-settings-panel");
    await panel.getByRole("tab", { name: "General", exact: true }).click();
    await expect(panel.locator("input").first()).toHaveValue(model);
    await expect(panel.locator("input").last()).toHaveValue(
      "https://api.fixture.example.com/v1/agent-runtime/configuration",
    );
    expect(
      await page.evaluate(() => window.agentManagementProbe.discoveredSettings),
    ).toContain(provider);
    expect(errors).toEqual([]);
  });
}

for (const [name, provider, mcpScopes] of [
  ["OpenCode", "opencode", ["user", "project"]],
  ["DeepSeek Harness", "deepseek_harness", ["user"]],
] as const) {
  test(`${name} exposes only advertised MCP scopes and writes Skills to the selected provider`, async ({
    page,
  }) => {
    await page.goto("/?signedOut=true");
    await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await page.getByRole("button", { name: "MCP", exact: true }).click();
    await page.getByRole("button", { name: "Add", exact: true }).click();
    const dialog = openDialog(page);
    await expect(
      dialog.getByRole("combobox", { name: "Provider", exact: true }),
    ).toHaveValue(provider);
    await expect(
      dialog.getByRole("combobox", { name: "Provider", exact: true }),
    ).toBeDisabled();
    expect(
      await dialog
        .getByRole("combobox", { name: "Scope", exact: true })
        .locator("option")
        .evaluateAll((options) =>
          options.map((option) => (option as HTMLOptionElement).value),
        ),
    ).toEqual(mcpScopes);
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();

    await page.getByRole("button", { name: "Skills", exact: true }).click();
    await page.getByRole("button", { name: "Add", exact: true }).click();
    expect(
      await dialog
        .getByRole("combobox", { name: "Scope", exact: true })
        .locator("option")
        .evaluateAll((options) =>
          options.map((option) => (option as HTMLOptionElement).value),
        ),
    ).toEqual(["user", "project"]);
    await dialog
      .getByLabel("Installation name", { exact: true })
      .fill("fixture-skill");
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    const writes = await page.evaluate(
      () => window.agentManagementProbe.createdTools,
    );
    expect(writes).toHaveLength(1);
    expect(writes[0].target).toMatchObject({
      provider,
      scope: "user",
      kind: "skill",
      name: "fixture-skill",
    });
    expect(writes[0].definition.type).toBe("skill");
  });
}

for (const scope of ["user", "project"] as const) {
  test(`OpenCode ${scope} MCP copy respects the target provider's advertised scope`, async ({
    page,
  }) => {
    await page.goto("/?signedOut=true");
    await page.getByRole("button", { name: /^OpenCode/ }).click();
    await page.getByRole("button", { name: "MCP", exact: true }).click();
    if (scope === "project") {
      await page
        .getByRole("textbox", { name: "Project path (optional)", exact: true })
        .fill("C:\\AgentCenterFixture");
      await page
        .getByRole("button", { name: "Discover project tools", exact: true })
        .click();
    }
    await page.getByRole("button", { name: "Add", exact: true }).click();
    const dialog = openDialog(page);
    await dialog
      .getByRole("combobox", { name: "Scope", exact: true })
      .selectOption(scope);
    await dialog
      .getByRole("textbox", { name: "Installation name", exact: true })
      .fill("fixture-mcp");
    await dialog
      .getByRole("textbox", {
        name: "Portable MCP definition (JSON)",
        exact: true,
      })
      .fill(
        JSON.stringify({
          transport: "stdio",
          command: { type: "replace", data: { value: "fixture-mcp" } },
          args: { type: "replace", data: { value: [] } },
          cwd: { type: "clear" },
          url: { type: "clear" },
          env: { type: "clear" },
          headers: { type: "clear" },
        }),
      );
    await dialog.getByRole("button", { name: "Add", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await page.getByRole("button", { name: "Copy", exact: true }).click();
    const targets = await dialog
      .getByRole("combobox", { name: "Target provider", exact: true })
      .locator("option")
      .evaluateAll((options) =>
        options.map((option) => (option as HTMLOptionElement).value),
      );
    if (scope === "user") expect(targets).toContain("deepseek_harness");
    else expect(targets).not.toContain("deepseek_harness");
    const writes = await page.evaluate(
      () => window.agentManagementProbe.createdTools,
    );
    expect(writes[0].target).toMatchObject({
      provider: "opencode",
      scope,
      kind: "mcp_server",
    });
    expect(writes[0].target.project_path).toBe(
      scope === "project" ? "C:\\AgentCenterFixture" : undefined,
    );
  });
}

test("OpenCode native commands are editable while DeepSeek Harness commands stay unsupported", async ({
  page,
}) => {
  await page.goto("/?signedOut=true");
  await page.getByRole("button", { name: /^DeepSeek Harness/ }).click();
  await page.getByRole("button", { name: "Commands", exact: true }).click();
  await expect(
    page.getByText("DeepSeek Harness does not expose native prompt commands.", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Add command", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Project path", { exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: /^OpenCode/ }).click();
  await page.getByRole("button", { name: "Add command", exact: true }).click();
  const dialog = openDialog(page);
  await expect(
    dialog.getByText("OpenCode Markdown", { exact: true }),
  ).toBeVisible();
  await dialog
    .getByRole("textbox", { name: /^Command name/ })
    .fill("fixture-command");
  await dialog
    .getByLabel("Description (optional)", { exact: true })
    .fill("Fixture description");
  await dialog
    .getByLabel("Prompt body", { exact: true })
    .fill("Describe the fixture changes.");
  await dialog
    .getByRole("button", { name: "Add command", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  const writes = await page.evaluate(
    () => window.agentManagementProbe.createdCommands,
  );
  expect(writes).toHaveLength(1);
  expect(writes[0].target).toMatchObject({
    provider: "opencode",
    scope: "user",
    name: "fixture-command",
  });
  expect(writes[0].definition).toEqual({
    type: "opencode",
    data: {
      description: { type: "replace", data: { value: "Fixture description" } },
      body: {
        type: "replace",
        data: { value: "Describe the fixture changes." },
      },
    },
  });
});

test("real model selector preserves an advertised empty variant and does not invent DSH effort", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?signedOut=true&selectors=true");
  const session = page.getByRole("region", {
    name: "Session agent selector",
    exact: true,
  });
  const model = session.getByRole("button", {
    name: /^FixtureProvider\/FixtureModel/,
  });
  await expect(model).toBeVisible();
  await model.click();
  await page
    .getByRole("button", { name: "Follow CLI config", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "No variant", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(model).toContainText("No variant");
  expect(
    JSON.parse(await page.getByTestId("session-selection").innerText())
      .reasoning_id,
  ).toBe("");
  const profiles = await page.evaluate(
    () => window.agentManagementProbe.savedProfiles,
  );
  expect(
    profiles.some((value) => value.includes('"":') || value.includes(': ""')),
  ).toBe(true);

  await session
    .getByRole("button", { name: "DEEPSEEK_HARNESS", exact: true })
    .click();
  await expect(
    session.getByRole("button", { name: "FixtureDshModel", exact: true }),
  ).toBeVisible();
  await session
    .getByRole("button", { name: "FixtureDshModel", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "No variant", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Follow CLI config", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("real workflow selector supports both new providers and reports the selected executor", async ({
  page,
}) => {
  await page.goto("/?signedOut=true&selectors=true");
  const workflow = page.getByRole("region", {
    name: "Workflow agent selector",
    exact: true,
  });
  await workflow
    .getByRole("button", { name: "Select executor", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "OpenCode", exact: true }).click();
  await expect(page.getByTestId("workflow-selection")).toContainText(
    '"executor":"OPENCODE"',
  );
  await workflow.getByRole("button", { name: "OpenCode", exact: true }).click();
  await page
    .getByRole("menuitem", { name: "DeepSeek Harness", exact: true })
    .click();
  await expect(page.getByTestId("workflow-selection")).toContainText(
    '"executor":"DEEPSEEK_HARNESS"',
  );
});

test("unavailable provider is disabled in session and real workflow selectors", async ({
  page,
}) => {
  await page.goto("/?signedOut=true&selectors=true&missing=DEEPSEEK_HARNESS");
  await expect(
    page
      .getByRole("region", { name: "Session agent selector" })
      .getByRole("button", { name: "DEEPSEEK_HARNESS", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("region", { name: "Workflow agent selector" })
    .getByRole("button", { name: "Select executor", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: /^DeepSeek Harness/ }),
  ).toHaveAttribute("aria-disabled", "true");
});
