import { expect, test } from "@playwright/test";

test("search keeps geometry stable, handles IME and scrolls keyboard selection", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");
  const trigger = page
    .locator(".vk-product-sidebar")
    .getByRole("button", { name: "Search" });
  await trigger.click();
  const dialog = page.getByRole("dialog", { name: "Global search" });
  const input = dialog.getByRole("combobox");
  await expect(input).toBeFocused();
  await expect(dialog).toHaveCSS("height", "600px");
  await input.fill("no such destination");
  await expect(dialog.getByRole("option")).toHaveCount(0);
  await expect(dialog).toHaveCSS("height", "600px");
  await input.fill("Codex");
  await input.dispatchEvent("keydown", { key: "Enter", isComposing: true });
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId("current-route")).toHaveText("/dashboard");
  await input.dispatchEvent("keydown", { key: "Escape", isComposing: true });
  await expect(dialog).toBeVisible();
  await input.fill("");
  await input.press("ArrowUp");
  const selected = dialog.locator('[role="option"][aria-selected="true"]');
  await expect(selected).toBeInViewport();
  await input.press("Tab");
  await expect(
    dialog.getByRole("button", { name: "Close search" }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(input).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  await trigger.click();
  await expect(input).toHaveValue("");
  await page.locator(".vk-search-overlay").click({ position: { x: 5, y: 5 } });
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("search supports pointer navigation and reduced motion", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "dark" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");
  await page
    .locator(".vk-product-sidebar")
    .getByRole("button", { name: "Search" })
    .click();
  const dialog = page.getByRole("dialog", { name: "Global search" });
  await dialog.getByRole("combobox").fill("Claude");
  const option = dialog.getByRole("option", { name: /^Claude Code/ });
  await option.hover();
  await expect(option).toHaveAttribute("aria-selected", "true");
  await option.click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("current-route")).toHaveText(
    "/agents?provider=claude_code",
  );
  await expect(page.locator("#main-content")).toBeFocused();
});
