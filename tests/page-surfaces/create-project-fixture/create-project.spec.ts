import { expect, test, type Page } from "@playwright/test";

test("runtime clients and caches stay with their captured host", async ({
  page,
}) => {
  await page.goto("/");
  const moduleRoot = `/@fs/${process.cwd().replaceAll("\\", "/")}/packages/web-core/src/shared`;
  const keys = await page.evaluate(async (root) => {
    const { createWorkflowApi } = await import(`${root}/lib/workflowApi.ts`);
    const { createArenaApi } = await import(`${root}/lib/arenaApi.ts`);
    const { createScheduledTaskApi } = await import(
      `${root}/lib/scheduledTaskApi.ts`
    );
    const { workflowTemplateQueryKeys } = await import(
      `${root}/hooks/useWorkflowTemplates.ts`
    );
    const { workflowAttemptQueryKeys } = await import(
      `${root}/hooks/useWorkflowAttempts.ts`
    );
    const { arenaQueryKeys } = await import(`${root}/hooks/useArenaGroup.ts`);
    const { scheduledTaskQueryKeys } = await import(
      `${root}/hooks/useScheduledTasks.ts`
    );
    await createWorkflowApi("host-one").list("same-project");
    await createArenaApi("host-two").getActiveForTask("same-task");
    await createScheduledTaskApi(null).list("same-project");
    return [
      workflowTemplateQueryKeys.list("same-project", "host-one"),
      workflowTemplateQueryKeys.list("same-project", "host-two"),
      workflowAttemptQueryKeys.task("same-project", "same-task", "host-one"),
      arenaQueryKeys.activeForTask("same-task", "host-two"),
      scheduledTaskQueryKeys.project("same-project", null),
    ];
  }, moduleRoot);
  expect(keys[0]).not.toEqual(keys[1]);
  expect(keys[2]).toContain("host-one");
  expect(keys[3]).toContain("host-two");
  expect(keys[4]).toContain(null);
  expect(await data(page, "requests")).toEqual([
    expect.objectContaining({
      path: "/api/local/v1/projects/same-project/workflows",
      hostScope: "explicit",
      hostId: "host-one",
    }),
    expect.objectContaining({
      path: "/api/local/v1/tasks/same-task/arena/active",
      hostScope: "explicit",
      hostId: "host-two",
    }),
    expect.objectContaining({ hostScope: "explicit", hostId: null }),
  ]);
});

async function open(page: Page, query = "") {
  await page.goto(`/${query}`);
  await page.getByRole("button", { name: "Open create project" }).click();
  await page.getByLabel("Project name").fill("New project");
}
const create = (page: Page) =>
  page.getByRole("button", { name: "Create Project", exact: true }).click();
const data = (page: Page, key: string) =>
  page
    .locator("html")
    .getAttribute(`data-${key}`)
    .then((value) => JSON.parse(value ?? "null"));

test("directory is required, including Enter; cancel writes nothing", async ({
  page,
}) => {
  await open(page);
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeDisabled();
  await page.getByLabel("Project name").press("Enter");
  await expect(page.getByRole("alert")).toContainText(
    "Choose a working directory",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await data(page, "inserts")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
  expect(await data(page, "result")).toMatchObject({ action: "canceled" });
});

for (const mode of ["folder", "git"]) {
  test(`${mode} saves default to exact created project and selected host`, async ({
    page,
  }) => {
    await open(page, `?mode=${mode}`);
    await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
    await page
      .getByRole("button", {
        name: "Choose folder or Git repository",
        exact: true,
      })
      .click();
    await expect(page.getByRole("status")).toContainText(
      mode === "git" ? "feature/test" : "/fixture/folder",
    );
    await create(page);
    await expect(page.getByRole("dialog")).not.toBeVisible();
    expect(await data(page, "picker")).toMatchObject([
      { hostId: "remote-1", purpose: "project_default" },
    ]);
    expect(await data(page, "saves")).toEqual([
      {
        projectId: "created-project",
        hostId: "remote-1",
        value:
          mode === "git"
            ? {
                kind: "git",
                repo: { repo_id: "repo-2", target_branch: "feature/test" },
              }
            : { kind: "direct_folder", path: "/fixture/folder" },
      },
    ]);
  });
}

test("save failure retries without creating duplicate project", async ({
  page,
}) => {
  await open(page, "?fail");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("alert")).toContainText(
    "The working directory could not be saved",
  );
  await page
    .getByRole("button", { name: "Retry saving directory", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "inserts")).toHaveLength(1);
  expect(await data(page, "saves")).toHaveLength(2);
});

test("cancel after failed save discards unfinished project instead of reporting success", async ({
  page,
}) => {
  await open(page, "?fail");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("alert")).toContainText(
    "The working directory could not be saved",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "result")).toMatchObject({
    action: "canceled",
  });
  expect(await data(page, "removes")).toEqual(["created-project"]);
  expect(await data(page, "inserts")).toHaveLength(1);
});

test("pending save locks cancellation and machine selection", async ({
  page,
}) => {
  await open(page, "?hold");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByLabel("Machine", { exact: true })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Cancel", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-save-release")),
  );
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "inserts")).toHaveLength(1);
});

test("changing machine requires choosing its directory again", async ({
  page,
}) => {
  await open(page);
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeEnabled();
  await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeDisabled();
  await page.getByLabel("Project name").press("Enter");
  expect(await data(page, "inserts")).toBeNull();
});

test("failed cancellation stays open and retries removing the same project", async ({
  page,
}) => {
  await open(page, "?fail&cancel-fail");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("alert")).toContainText(
    "The working directory could not be saved",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Could not discard");
  expect(await data(page, "result")).toBeNull();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "removes")).toEqual([
    "created-project",
    "created-project",
  ]);
  expect(await data(page, "inserts")).toHaveLength(1);
  expect(await data(page, "result")).toEqual({ action: "canceled" });
});

test("picker cancellation does not satisfy the required directory", async ({
  page,
}) => {
  await open(page, "?picker-cancel");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeDisabled();
  expect(await data(page, "inserts")).toBeNull();
});

test("host going offline prevents creation even by Enter", async ({ page }) => {
  await open(page);
  await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-host-offline")),
  );
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeDisabled();
  await page.getByLabel("Project name").press("Enter");
  await expect(page.getByRole("alert")).toContainText(
    "Choose the working directory again",
  );
  expect(await data(page, "inserts")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
});

test("new local directories use the created identity and selected host without another insertion", async ({
  page,
}) => {
  await open(page, "?local");
  await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "inserts")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
  expect(await data(page, "opens")).toEqual([
    {
      input: {
        directory_path: "/fixture/folder",
        name: "New project",
        color: expect.any(String),
      },
      hostId: "remote-1",
    },
  ]);
  expect(await data(page, "result")).toMatchObject({
    action: "created",
    hostId: "remote-1",
    project: { id: "directory-project", name: "New project" },
  });
});

test("failed Git enrichment retries the new directory project without opening a duplicate", async ({
  page,
}) => {
  await open(page, "?local&mode=git&fail");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("alert")).toContainText("The project was opened");
  await page
    .getByRole("button", { name: "Retry saving directory", exact: true })
    .click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "saves")).toEqual([
    expect.objectContaining({ projectId: "directory-project" }),
    expect.objectContaining({ projectId: "directory-project" }),
  ]);
});

test("cancel after a local directory open failure never deletes project files", async ({
  page,
}) => {
  await open(page, "?local&mode=git&fail");
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
  await create(page);
  await expect(page.getByRole("alert")).toContainText(
    "Cancel keeps the project and its files",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  expect(await data(page, "removes")).toBeNull();
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "result")).toEqual({ action: "canceled" });
});

const existingProjectId = "68916c8e-7f85-48f1-b15e-a63cb87f4e45";
const existingConfirmation = (page: Page) =>
  page.getByRole("dialog", { name: "Open existing project?", exact: true });
const creationDialog = (page: Page) =>
  page.getByRole("dialog", { name: "Create Project", exact: true });

async function chooseDirectory(page: Page) {
  await page
    .getByRole("button", {
      name: "Choose folder or Git repository",
      exact: true,
    })
    .click();
}

test("canceling existing-directory confirmation keeps the draft and does not reopen or mutate it", async ({
  page,
}) => {
  await open(page, "?local&existing");
  await chooseDirectory(page);
  await create(page);
  const confirmation = existingConfirmation(page);
  await expect(confirmation).toContainText("Portable project");
  await expect(confirmation).toContainText(
    "the details entered for the new project will not be used",
  );
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "inserts")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
  await confirmation
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(page.getByLabel("Project name")).toHaveValue("New project");
  await expect(page.getByRole("status")).toContainText("/fixture/folder");
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeEnabled();
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "refreshes")).toBeNull();
  expect(await data(page, "removes")).toBeNull();
  expect(await data(page, "result")).toBeNull();
});

for (const mode of ["folder", "git"]) {
  test(`confirmed ${mode} opens the displayed identity on its captured Host and preserves old settings`, async ({
    page,
  }) => {
    await open(page, `?local&existing&mode=${mode}&fail`);
    await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
    await chooseDirectory(page);
    await create(page);
    await existingConfirmation(page)
      .getByRole("button", { name: "Open existing project", exact: true })
      .click();
    await expect(creationDialog(page)).not.toBeVisible();
    const opens = await data(page, "opens");
    expect(opens).toHaveLength(2);
    expect(opens[0]).toMatchObject({
      hostId: "remote-1",
      input: {
        directory_path: mode === "git" ? "/fixture/repo" : "/fixture/folder",
      },
    });
    expect(opens[0].input).not.toHaveProperty("confirmed_project_id");
    expect(opens[1]).toEqual({
      hostId: "remote-1",
      input: { ...opens[0].input, confirmed_project_id: existingProjectId },
    });
    expect(await data(page, "inserts")).toBeNull();
    expect(await data(page, "saves")).toBeNull();
    expect(await data(page, "removes")).toBeNull();
    expect(await data(page, "result")).toMatchObject({
      action: "created",
      hostId: "remote-1",
      project: {
        id: existingProjectId,
        name: "Portable project",
        color: "100 20% 30%",
      },
    });
  });
}

test("changed directory identity after confirmation is not silently confirmed again", async ({
  page,
}) => {
  await open(page, "?local&existing&identity-changed");
  await chooseDirectory(page);
  await create(page);
  await existingConfirmation(page)
    .getByRole("button", { name: "Open existing project", exact: true })
    .click();
  await expect(existingConfirmation(page)).not.toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Confirm before opening");
  expect(await data(page, "opens")).toHaveLength(2);
  expect(await data(page, "result")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
  expect(await data(page, "removes")).toBeNull();
});

for (const conflict of [
  "ordinary-conflict",
  "message-conflict",
  "text-conflict",
  ...["id", "name", "code", "success", "status"].map(
    (field) => `existing&malformed-conflict=${field}`,
  ),
]) {
  test(`${conflict} is an ordinary error, not permission to restore a project`, async ({
    page,
  }) => {
    await open(page, `?local&${conflict}`);
    await chooseDirectory(page);
    await create(page);
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(existingConfirmation(page)).not.toBeVisible();
    expect(await data(page, "opens")).toHaveLength(1);
    expect(await data(page, "saves")).toBeNull();
    expect(await data(page, "result")).toBeNull();
  });
}

test("an unavailable machine cannot be confirmed or replaced by this machine", async ({
  page,
}) => {
  await open(page, "?local&existing");
  await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
  await chooseDirectory(page);
  await create(page);
  const confirmation = existingConfirmation(page);
  await expect(confirmation).toBeVisible();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-host-offline")),
  );
  await confirmation
    .getByRole("button", { name: "Open existing project", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText(
    "Choose the working directory again",
  );
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "saves")).toBeNull();
  expect(await data(page, "result")).toBeNull();
});

test("machine switching invalidates a pending confirmation without sending it to either machine", async ({
  page,
}) => {
  await open(page, "?local&existing");
  await page.getByLabel("Machine", { exact: true }).selectOption("remote-1");
  await chooseDirectory(page);
  await create(page);
  const confirmation = existingConfirmation(page);
  await expect(confirmation).toBeVisible();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-host-switch")),
  );
  await confirmation
    .getByRole("button", { name: "Open existing project", exact: true })
    .click();
  await expect(page.getByLabel("Machine", { exact: true })).toHaveValue(
    "local",
  );
  await expect(
    page.getByRole("button", { name: "Create Project", exact: true }),
  ).toBeDisabled();
  expect(await data(page, "opens")).toHaveLength(1);
  expect(await data(page, "result")).toBeNull();
  expect(await data(page, "refreshes")).toBeNull();
});

test("confirmation stays single-flight while the confirmed open request is pending", async ({
  page,
}) => {
  await open(page, "?local&existing&hold-confirmed-open");
  await chooseDirectory(page);
  await create(page);
  await existingConfirmation(page)
    .getByRole("button", { name: "Open existing project", exact: true })
    .click();
  await expect(page.getByLabel("Machine", { exact: true })).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Cancel", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(creationDialog(page)).toBeVisible();
  expect(await data(page, "opens")).toHaveLength(2);
  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-open-release")),
  );
  await expect(creationDialog(page)).not.toBeVisible();
  expect(await data(page, "opens")).toHaveLength(2);
});

test("failed explicit reopen can be canceled without deleting the portable project", async ({
  page,
}) => {
  await open(page, "?local&existing&confirmed-open-fails");
  await chooseDirectory(page);
  await create(page);
  await existingConfirmation(page)
    .getByRole("button", { name: "Open existing project", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not open the project",
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await data(page, "removes")).toBeNull();
  expect(await data(page, "saves")).toBeNull();
  expect(await data(page, "result")).toEqual({ action: "canceled" });
});

test("local deletion explains retained files and explicit reopen; cloud keeps permanent deletion semantics", async ({
  page,
}) => {
  for (const deployment of ["local", "cloud"]) {
    await page.goto(deployment === "local" ? "/?local" : "/");
    await page
      .getByRole("button", { name: "Open delete project", exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "Delete Project?" });
    await expect(dialog).toContainText("Portable project");
    if (deployment === "local") {
      await expect(dialog).toContainText("files, and task data will be kept");
      await expect(dialog).toContainText("explicitly reopen its directory");
      await expect(dialog).not.toContainText("permanently delete");
    } else {
      await expect(dialog).toContainText("permanently delete");
      await expect(dialog).not.toContainText(
        "files, and task data will be kept",
      );
    }
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(dialog).not.toBeVisible();
    await expect(page.locator("html")).not.toHaveAttribute(
      "data-deletion-requested",
    );
  }
});
