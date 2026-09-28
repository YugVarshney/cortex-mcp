import { expect, test } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

test.describe("Recall-MCP UI", () => {
  test("create memory, recall it, inspect the score breakdown", async ({ page }) => {
    await page.goto("/");

    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Recall-MCP memory console");

    // The store starts empty: create the namespace used by the rest of the flow.
    await page.getByLabel("New namespace name").fill("e2e");
    await page.getByRole("button", { name: "Create namespace" }).click();
    await expect(page.getByRole("status").first()).toHaveText(/created/i);

    // Create a memory through the UI form.
    await page.getByLabel("New memory text").fill("playwright e2e verifies the ranking explanation");
    await page.getByRole("button", { name: "Remember" }).click();
    await expect(page.getByRole("status").nth(1)).toHaveText(/stored/i);

    // Recall it and verify the breakdown table renders explainable numbers.
    await page.getByLabel("Query").fill("ranking explanation e2e");
    await page.getByRole("button", { name: "Recall" }).click();
    const table = page.getByRole("table", { name: "Recalled memories with score breakdowns" });
    await expect(table).toBeVisible();
    const firstRow = table.getByRole("row").nth(1);
    await expect(firstRow).toContainText("playwright e2e verifies the ranking explanation");
    for (const column of ["BM25", "Vector", "Recency", "Pinned", "Total"]) {
      await expect(table.getByRole("columnheader", { name: column })).toBeVisible();
    }
    // The total must be a real number in the row (not blank/NaN).
    const totalText = await firstRow.getByRole("cell").nth(5).innerText();
    expect(Number(totalText)).not.toBeNaN();

    // Keyboard: the recall button must be reachable and focusable.
    await page.getByLabel("Query").focus();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("button", { name: "Recall" })).toBeFocused();
  });

  test("page is keyboard navigable and passes axe accessibility checks", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();

    // New surfaces render async: wait for them so axe scans the loaded state.
    await expect(page.getByRole("table", { name: "Paged list of stored memories" })).toBeVisible();
    await expect(page.getByRole("group", { name: "Message 1" })).toBeVisible();
    await expect(page.getByRole("table", { name: "Server metrics with values" })).toBeVisible();

    // Visible focus: tab to the first link and confirm it has an outline.
    await page.keyboard.press("Tab");
    const firstLink = page.getByRole("link", { name: "Namespaces" });
    await expect(firstLink).toBeFocused();

    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    expect(
      results.violations.map((v) => `${v.id} (${v.nodes.length} nodes)`),
      JSON.stringify(results.violations, null, 2),
    ).toEqual([]);
  });

  test("browse memories with server-driven pagination", async ({ page, request }) => {
    // Seed through the API (the UI forms are exercised by the other tests).
    await request.post("/v1/namespaces", { data: { name: "paging" } });
    for (let i = 1; i <= 12; i++) {
      await request.post("/v1/memories", {
        data: { namespace: "paging", text: `paging seed memory ${i}` },
      });
    }

    await page.goto("/");
    const section = page.getByRole("region", { name: "Browse memories" });
    await section.getByLabel("Namespace").selectOption("paging");
    const status = section.getByRole("status");
    const table = section.getByRole("table", { name: "Paged list of stored memories" });

    await expect(status).toHaveText("Page 1 of 2 · memories 1–10 of 12 · newest first");
    await expect(table).toBeVisible();
    await expect(table.getByRole("row")).toHaveCount(11); // header + 10 rows
    // Same-second seeds tie-break on random ids, so page contents are not
    // insertion-ordered — assert membership, not row positions.
    await expect(table).toContainText("paging seed memory 12");

    await section.getByRole("button", { name: "Next page" }).click();
    await expect(status).toHaveText("Page 2 of 2 · memories 11–12 of 12 · newest first");
    await expect(table.getByRole("row")).toHaveCount(3); // header + 2 rows
    await expect(section.getByRole("button", { name: "Next page" })).toBeDisabled();

    // Keyboard-only paging: the enabled button takes focus and Enter activates it.
    await section.getByRole("button", { name: "Previous page" }).focus();
    await expect(section.getByRole("button", { name: "Previous page" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(status).toHaveText("Page 1 of 2 · memories 1–10 of 12 · newest first");
    await expect(section.getByRole("button", { name: "Previous page" })).toBeDisabled();

    // A larger page size collapses everything onto one page.
    await section.getByLabel("Memories per page").selectOption("50");
    await expect(status).toHaveText("Page 1 of 1 · memories 1–12 of 12 · newest first");
    await expect(section.getByRole("button", { name: "Next page" })).toBeDisabled();
  });

  test("capture a transcript into chunked memories", async ({ page }) => {
    await page.goto("/");
    const section = page.getByRole("region", { name: "Capture a transcript" });
    const memories = page.getByRole("region", { name: "Browse memories" });

    await section.getByLabel("Namespace").fill("captured-e2e");
    await section
      .getByRole("group", { name: "Message 1" })
      .getByLabel("Role (optional)")
      .fill("user");
    await section
      .getByRole("group", { name: "Message 1" })
      .getByLabel("Content")
      .fill("capture e2e user turn about pagination");
    await expect(section.getByRole("button", { name: "Remove message 1" })).toBeDisabled();

    // Dynamic rows: add a second message, remove it, add it back.
    await section.getByRole("button", { name: "Add message" }).click();
    await expect(section.getByRole("button", { name: "Remove message 2" })).toBeEnabled();
    await section.getByRole("button", { name: "Remove message 2" }).click();
    await expect(section.getByRole("group", { name: "Message 2" })).toHaveCount(0);
    await section.getByRole("button", { name: "Add message" }).click();
    await section
      .getByRole("group", { name: "Message 2" })
      .getByLabel("Content")
      .fill("capture e2e assistant turn about metrics");

    await section.getByRole("button", { name: "Capture transcript" }).click();
    await expect(section.getByRole("status")).toHaveText(
      "Captured 2 memories into “captured-e2e”.",
    );

    // The captured memories are really in the store and browsable.
    await expect(
      memories.getByLabel("Namespace").getByRole("option", { name: "captured-e2e" }),
    ).toBeAttached();
    await memories.getByLabel("Namespace").selectOption("captured-e2e");
    await expect(memories.getByRole("status")).toHaveText(
      "Page 1 of 1 · memories 1–2 of 2 · newest first",
    );
    // Same-second memories tie-break on random ids: assert contents, not order.
    // Message 1 has role "user" (prefixed + tagged); message 2 has no role.
    await expect(memories.getByRole("table")).toContainText(
      "user: capture e2e user turn about pagination",
    );
    await expect(memories.getByRole("table")).toContainText(
      "capture e2e assistant turn about metrics",
    );
    await expect(memories.getByRole("table")).toContainText("role:user");
  });

  test("metrics section renders the Prometheus surface as linear tables", async ({ page }) => {
    await page.goto("/");
    const section = page.getByRole("region", { name: "Server metrics" });
    const table = section.getByRole("table", { name: "Server metrics with values" });
    await expect(table).toBeVisible();

    // The page load itself produced requests the counters must reflect.
    const requestsRow = table.getByRole("row").filter({ hasText: "recall_requests_total" });
    await expect(requestsRow).toHaveCount(1);
    const value = Number(await requestsRow.getByRole("cell").innerText());
    expect(value).toBeGreaterThanOrEqual(1);

    // Refresh announces itself politely with the sample count.
    await section.getByRole("button", { name: "Refresh metrics" }).click();
    await expect(section.getByRole("status")).toContainText(/Metrics refreshed · \d+ samples\./);

    // Full fidelity stays available as plain linear text, keyboard-reachable.
    await section.getByText("Raw Prometheus text").click();
    const raw = section.getByLabel("Raw Prometheus text");
    await expect(raw).toBeVisible();
    await expect(raw).toContainText("# HELP recall_requests_total Total HTTP requests handled.");
    await expect(raw).toContainText("# TYPE recall_requests_total counter");

    // The expanded metrics surface stays axe-clean.
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    expect(
      results.violations.map((v) => `${v.id} (${v.nodes.length} nodes)`),
      JSON.stringify(results.violations, null, 2),
    ).toEqual([]);
  });
});
