import { expect, test } from "@playwright/test";

const viewports = [
  { width: 375, height: 812 },
  { width: 768, height: 812 },
  { width: 1024, height: 812 },
  { width: 1440, height: 900 },
];

async function gotoFixture(page: import("@playwright/test").Page) {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Canonical task title" }),
  ).toBeVisible();
}

test.describe("P8-R1 Agent Workbench geometry", () => {
  for (const viewport of viewports) {
    test(`${viewport.width}px keeps conversation and controls within the viewport`, async ({
      page,
    }) => {
      await page.setViewportSize(viewport);
      await gotoFixture(page);

      await expect
        .poll(() =>
          page.evaluate(() => ({
            viewport: window.innerWidth,
            document: document.documentElement.scrollWidth,
            body: document.body.scrollWidth,
          })),
        )
        .toEqual({
          viewport: viewport.width,
          document: viewport.width,
          body: viewport.width,
        });

      const composer = page.locator(".vk-agent-workbench-composer");
      const composerBox = await composer.boundingBox();
      if (!composerBox) throw new Error("Composer geometry is unavailable");
      expect(composerBox.x).toBeGreaterThanOrEqual(0);
      expect(composerBox.x + composerBox.width).toBeLessThanOrEqual(
        viewport.width,
      );
      expect(composerBox.y + composerBox.height).toBeLessThanOrEqual(
        viewport.height,
      );

      await expect(
        page.getByRole("button", { name: "Inspector", exact: true }),
      ).toBeVisible();
      await page
        .getByRole("button", { name: "Inspector", exact: true })
        .click();
      const close = page.getByRole("button", { name: "Close Inspector" });
      await expect(close).toBeVisible();
      const closeBounds = (await close.boundingBox())!;
      expect(closeBounds.height).toBeGreaterThanOrEqual(44);
      expect(closeBounds.width).toBeGreaterThanOrEqual(44);
    });
  }

  test("composer clears the virtual-keyboard viewport without clipping", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await gotoFixture(page);
    const editor = page.getByPlaceholder("Message session-1");
    await editor.fill("keyboard viewport check");

    // A mobile virtual keyboard reduces the visual viewport. Reflow must keep
    // the fixed composer inside that reduced space and preserve the draft.
    await page.setViewportSize({ width: 390, height: 420 });
    const composer = page.locator(".vk-agent-workbench-composer");
    const box = await composer.boundingBox();
    if (!box) throw new Error("Composer did not render after viewport resize");
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(420);
    await expect(editor).toHaveValue("keyboard viewport check");
  });
});
