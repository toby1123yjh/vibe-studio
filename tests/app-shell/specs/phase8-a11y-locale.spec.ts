import { expect, test } from "@playwright/test";
import { mockEmptyDashboard } from "../support/dashboard-api";

test.beforeEach(async ({ page }) => mockEmptyDashboard(page));

const locales = ["en", "es", "fr", "ja", "ko", "zh-Hans", "zh-Hant"];

for (const locale of locales) {
  test(`Dashboard keeps a labelled table and translated or fallback filters in ${locale}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto(`/?view=dashboard&locale=${encodeURIComponent(locale)}`);

    await expect(page.locator("html")).toHaveAttribute(
      "data-fixture-locale",
      locale,
    );
    const dashboard = page.getByRole("region", {
      name: "Dashboard",
    });
    await expect(dashboard).toBeVisible();
    await expect(dashboard.getByRole("heading", { level: 1 })).toHaveCount(1);
    await expect(dashboard.getByRole("table")).toHaveAccessibleName(
      locale === "zh-Hans" ? "智能体工作监控" : "Agent work monitoring",
    );
    await expect(dashboard.getByRole("combobox")).toHaveCount(0);
    const filters = dashboard.getByRole("button", {
      name: locale === "zh-Hans" ? "筛选" : "Filters",
      exact: true,
    });
    await filters.click();
    const filterPanel = page.getByRole("dialog", {
      name: locale === "zh-Hans" ? "筛选" : "Filters",
      exact: true,
    });
    await expect(filterPanel.getByRole("combobox")).toHaveCount(4);
    await expect(filterPanel.getByRole("checkbox")).toHaveAccessibleName(
      locale === "zh-Hans" ? "按项目分组" : "Group by project",
    );
    await page.keyboard.press("Escape");
    await expect(filters).toBeFocused();
    await expect(dashboard.locator('[data-state="unavailable"]')).toHaveCount(
      0,
    );
    await expect(dashboard).not.toContainText("sessionDashboard.");

    await page.evaluate(() => {
      document.documentElement.style.zoom = "2";
    });
    await expect(dashboard).toBeVisible();
    await expect(dashboard.getByRole("heading", { level: 1 })).toBeVisible();
  });
}
