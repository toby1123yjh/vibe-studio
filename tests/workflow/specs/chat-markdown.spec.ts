import { expect, test } from "@playwright/test";
import type {} from "../fixture/src/chat-markdown";

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.route("https://fonts.googleapis.com/**", (route) => route.abort());
  await page.goto("/chat-markdown.html");
  await expect(page.getByTestId("reply").locator("strong")).toHaveText(
    "answer",
  );
});

test("ordinary Markdown renders directly with safe links, file actions and copy", async ({
  page,
  context,
}) => {
  const content =
    "# Heading\n\nA **bold** reply with *emphasis*, ~~removed~~ and `code.ts`.\n\n- first\n- second\n\n| Item | Value |\n| --- | --- |\n| name | answer |\n\n```typescript\nconst answer = 42;\n```\n\n```\ncode.ts\n```\n\n[External](https://example.com) [Relative](./code.ts) [Unsafe](javascript:alert%281%29)\n\n<script>window.chatInjected = true</script>";
  await page.evaluate(
    (value) => window.chatMarkdownFixture.replace(value),
    content,
  );
  const reply = page.getByTestId("reply");
  await expect(reply.getByRole("heading", { name: "Heading" })).toBeVisible();
  await expect(reply.locator("table td").last()).toHaveText("answer");
  await expect(
    reply.locator("pre").first().locator(".hljs-keyword"),
  ).toHaveText("const");
  await expect(reply.locator("li")).toHaveCount(2);
  await expect(reply.getByRole("link", { name: "External" })).toHaveAttribute(
    "rel",
    "noopener noreferrer",
  );
  await expect(reply.getByRole("link", { name: "Relative" })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await expect(reply.getByRole("link", { name: "Unsafe" })).not.toHaveAttribute(
    "href",
  );
  await expect(reply.locator("script")).toHaveCount(0);
  await expect(reply.locator("[data-lexical-editor]")).toHaveCount(0);
  await expect(
    reply.getByRole("button", { name: "code.ts", exact: true }),
  ).toHaveCount(1);
  await reply.getByRole("button", { name: "code.ts", exact: true }).click();
  expect(
    await page.evaluate(() => window.chatMarkdownFixture.selectedPaths),
  ).toEqual(["src/code.ts"]);

  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await reply.hover();
  await reply.getByRole("button", { name: "Copy as Markdown" }).click();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  // The Windows clipboard normalizes line endings to CRLF.
  expect(copied.replace(/\r\n/g, "\n")).toBe(content);
});

test("streaming ordinary replies leaves completed history and the real input editor mounted", async ({
  page,
}) => {
  await page.evaluate(() => window.chatMarkdownFixture.setHistory(50));
  await expect(page.locator("[data-history-row]")).toHaveCount(50);
  // This is the production Lexical composer, not a replacement textarea. The
  // transcript must add zero editor roots even with substantial history.
  await expect(page.locator("[data-lexical-editor]")).toHaveCount(1);
  const firstHistory = await page
    .locator('[data-history-row="0"] p')
    .first()
    .elementHandle();
  const editor = page
    .getByTestId("composer")
    .locator('[contenteditable="true"]');
  const originalEditor = await editor.elementHandle();
  await editor.click();
  await page.evaluate(() => window.chatMarkdownFixture.startStream());
  await editor.pressSequentially(
    "Preparing the next message while the reply is arriving.",
    { delay: 15 },
  );
  await expect(page.getByTestId("reply")).toContainText("reply 20");
  await expect(page.getByTestId("draft")).toHaveText(
    "Preparing the next message while the reply is arriving.",
  );
  await expect(editor).toBeFocused();
  expect(await originalEditor!.evaluate((node) => node.isConnected)).toBe(true);
  expect(await firstHistory!.evaluate((node) => node.isConnected)).toBe(true);
  await expect(page.locator("[data-lexical-editor]")).toHaveCount(1);
});

test("rich image and review payloads retain the established presentation", async ({
  page,
}) => {
  await page.route("https://example.com/image.png", (route) =>
    route.fulfill({
      contentType: "image/png",
      body: Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jzV0AAAAASUVORK5CYII=",
        "base64",
      ),
    }),
  );
  const reply = page.getByTestId("reply");
  await page.route(
    "**/api/workspaces/markdown-workspace/attachments/metadata?**",
    (route) => {
      const url = new URL(route.request().url());
      expect(url.searchParams.get("session_id")).toBe("markdown-session");
      return route.fulfill({
        json: {
          success: true,
          data: {
            exists: true,
            file_name: "image.png",
            format: "png",
            proxy_url: "https://example.com/image.png",
          },
        },
      });
    },
  );
  await page.evaluate(() =>
    window.chatMarkdownFixture.replace(
      "![Existing preview](.vibe-attachments/image.png)",
    ),
  );
  await expect(reply.locator("[data-lexical-editor]")).toHaveCount(1);
  await expect(
    reply.getByRole("img", { name: "Existing preview" }),
  ).toBeVisible();
  await reply.getByRole("img", { name: "Existing preview" }).click();
  await expect(
    page.getByRole("heading", { name: "image.png", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: "Existing preview" })).toHaveCount(
    2,
  );
  await expect(
    page.getByRole("img", { name: "Existing preview" }).last(),
  ).toBeVisible();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.getByRole("img", { name: "Existing preview" })).toHaveCount(
    1,
  );

  const review = {
    id: "review-1",
    comment_type: "general",
    author: "reviewer",
    body: "Keep the review presentation",
    created_at: "2026-11-03T00:00:00Z",
  };
  await page.evaluate(
    (review) =>
      window.chatMarkdownFixture.replace(
        "```gh-comment\n" + JSON.stringify(review) + "\n```",
      ),
    review,
  );
  await expect(reply).toContainText("Keep the review presentation");
  await expect(reply).toContainText("reviewer");
  await expect(reply.locator("[data-lexical-editor]")).toHaveCount(1);
  await expect(reply.locator("pre")).toHaveCount(0);
});
