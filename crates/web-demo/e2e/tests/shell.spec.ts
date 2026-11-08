import { expect, test, type Page } from "@playwright/test";

const provenance = "Interactive Rust/WASM demo · fictional conversations · offline";

async function openPoster(page: Page) {
  await page.goto("index.html");
}

async function openDomDemo(page: Page) {
  await openPoster(page);
  await page.getByRole("button", { name: "Load interactive demo" }).click();
  await page.getByRole("link", { name: "Use accessible DOM renderer" }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");
  await expect(page.frameLocator("#demo-frame").locator("#demo")).toBeVisible();
  await expect(page.frameLocator("#demo-frame").locator("#demo canvas, #demo_ratzilla_grid")).toBeFocused();
}

test("poster stays readable and does not fetch WebAssembly before Load", async ({ page }) => {
  const wasmRequests: string[] = [];
  page.on("request", (request) => {
    if (/\.wasm(?:$|\?)/.test(request.url())) wasmRequests.push(request.url());
  });
  await openPoster(page);

  expect(new URL(page.url()).pathname).toBe("/televim/index.html");
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "poster");
  await expect(page.locator("#poster-frame")).not.toBeEmpty();
  await expect(page.getByText(provenance).first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Load interactive demo" })).toBeVisible();
  await expect(page.locator("#reset")).toBeHidden();
  expect(wasmRequests).toEqual([]);
});

test("Load requests one module and exposes running state with provenance", async ({ page }) => {
  const wasmRequests: string[] = [];
  page.on("request", (request) => {
    if (/\.wasm(?:$|\?)/.test(request.url())) wasmRequests.push(request.url());
  });
  await openPoster(page);
  await page.getByRole("button", { name: "Load interactive demo" }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");

  expect(wasmRequests).toHaveLength(1);
  await expect(page.locator("#demo-frame")).toBeVisible();
  await expect(page.getByText(provenance).first()).toBeVisible();
  expect(await page.evaluate(() => performance.getEntriesByName("televim-ready", "mark").length)).toBe(1);
});

test("keyboard opens prompts and insert mode; bare Escape stays in demo", async ({ page }) => {
  await openDomDemo(page);
  const terminal = page.frameLocator("#demo-frame").locator("#demo");
  await page.keyboard.press(":");
  await expect(terminal).toContainText(":");
  await page.keyboard.press("Escape");
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");

  await page.keyboard.press("i");
  await page.keyboard.type("ime-ready");
  await expect(terminal).toContainText("ime-ready");
});

test("Reset restores the fixture after a message is sent", async ({ page }) => {
  await openDomDemo(page);
  const terminal = page.frameLocator("#demo-frame").locator("#demo");
  await page.keyboard.press("i");
  await page.keyboard.type("reset-marker");
  await page.keyboard.press("Enter");
  await expect(terminal).toContainText("reset-marker");

  await page.getByRole("button", { name: "Reset demo" }).click();
  await expect(terminal).not.toContainText("reset-marker");
  await expect(terminal).toContainText("Hey, is the build green?");
});

test("Release returns keyboard focus to page and bare Escape does not release", async ({ page }) => {
  await openDomDemo(page);
  await page.keyboard.press("Escape");
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");

  await page.getByRole("button", { name: "Release keyboard" }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "released");
  await expect(page.getByRole("button", { name: /Click to interact/ })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.locator(":focus")).not.toHaveAttribute("id", "demo-frame");
});

test("mobile controls send text and release keyboard without horizontal page overflow", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDomDemo(page);
  const terminal = page.frameLocator("#demo-frame").locator("#demo");
  await page.getByRole("button", { name: "j", exact: true }).click();
  await page.getByRole("button", { name: "k", exact: true }).click();
  await page.getByRole("button", { name: "i", exact: true }).click();
  await page.getByRole("button", { name: "Type text" }).click();
  const mobileInput = page.getByRole("textbox", { name: "Type into Televim" });
  await mobileInput.fill("mobile-marker");
  await mobileInput.dispatchEvent("input", { inputType: "insertText", data: "mobile-marker" });
  await expect(terminal).toContainText("mobile-marker");
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "released");

  const sizes = await page.evaluate(() => ({
    body: document.body.scrollWidth,
    viewport: document.documentElement.clientWidth,
  }));
  expect(sizes.body).toBeLessThanOrEqual(sizes.viewport);
  await expect(page.getByRole("group", { name: "Extra terminal keys" })).toBeVisible();
});

test("synthetic IME commit is not duplicated, and same text typed later is accepted", async ({ page }) => {
  await openDomDemo(page);
  const terminal = page.frameLocator("#demo-frame").locator("#demo");
  await page.keyboard.press("i");
  await page.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>("#mobile-input")!;
    input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    input.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "あ" }));
    input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "あ" }));
    input.value = "あ";
    input.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      inputType: "insertText",
      data: "あ",
    }));
  });
  await expect(terminal).toContainText("あ");
  await page.waitForTimeout(20);

  await page.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>("#mobile-input")!;
    input.value = "あ";
    input.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      inputType: "insertText",
      data: "あ",
    }));
  });
  await expect(terminal).toContainText("ああ");
});

test("Ctrl+Alt+Escape releases keyboard and Click to interact restores it", async ({ page }) => {
  await openDomDemo(page);
  await page.keyboard.press("Control+Alt+Escape");
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "released");
  await expect(page.getByRole("button", { name: /Click to interact/ })).toBeFocused();

  await page.getByRole("button", { name: /Click to interact/ }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");
  await expect(page.frameLocator("#demo-frame").locator("#demo canvas, #demo_ratzilla_grid")).toBeFocused();
  await page.keyboard.press("i");
  await page.keyboard.type("focus-restored");
  await expect(page.frameLocator("#demo-frame").locator("#demo")).toContainText("focus-restored");
});

test("new-chat search resolves a fixture contact and opens its conversation", async ({ page }) => {
  await openDomDemo(page);
  const terminal = page.frameLocator("#demo-frame").locator("#demo");
  await page.keyboard.press("h");
  await page.keyboard.press("/");
  await page.keyboard.type("Juniper");
  await expect(terminal).toContainText("Juniper Reed");
  await page.keyboard.press("Enter");
  await expect(terminal).toContainText("Juniper Reed");
});

test("clipboard success reports copied state", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(Clipboard.prototype, "writeText", {
      configurable: true,
      value: async () => undefined,
    });
  });
  await openDomDemo(page);
  await page.keyboard.press("g");
  await page.keyboard.press("g");
  await page.keyboard.press("V");
  await page.keyboard.press("j");
  await page.keyboard.press("y");
  await expect(page.frameLocator("#demo-frame").getByText("Copied to clipboard.")).toBeVisible();
});

test("clipboard denial selects text and shows the exact manual-copy fallback", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(Clipboard.prototype, "writeText", {
      configurable: true,
      value: () => Promise.reject(new DOMException("denied", "NotAllowedError")),
    });
  });
  await openDomDemo(page);
  await page.keyboard.press("g");
  await page.keyboard.press("g");
  await page.keyboard.press("V");
  await page.keyboard.press("j");
  await page.keyboard.press("y");
  const dialog = page.frameLocator("#demo-frame").getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Clipboard access was denied. The text is selected; press Ctrl/Cmd+C.");
  await expect(dialog.getByRole("textbox", { name: "Text to copy" })).toHaveValue(/Hey, is the build green/);
});

test("unsupported browser keeps poster available", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "WebAssembly", { configurable: true, value: undefined });
  });
  await openPoster(page);
  await page.getByRole("button", { name: "Load interactive demo" }).click();

  await expect(page.locator("#plate")).toHaveAttribute("data-state", "unsupported");
  await expect(page.getByRole("heading", { name: "This browser can't run the demo" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "This browser can't run the demo" })).toBeFocused();
  await expect(page.locator("#poster-frame")).not.toBeEmpty();
});

test("failed module shows Retry and never claims to be running", async ({ page }) => {
  await page.route("**/demo.html**", (route) => route.fulfill({
    status: 200,
    contentType: "text/html",
    body: "<script>parent.postMessage({type:'failed',message:'simulated startup failure'},location.origin)</script>",
  }));
  await openPoster(page);
  await page.getByRole("button", { name: "Load interactive demo" }).click();

  await expect(page.locator("#plate")).toHaveAttribute("data-state", "failed");
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(page.locator("#demo-frame")).toBeHidden();
});

test("page identifies fictional offline provenance and makes no Telegram or storage calls", async ({ page }) => {
  const requests: string[] = [];
  page.on("request", (request) => requests.push(request.url()));
  await openPoster(page);
  await expect(page.getByText(provenance).first()).toBeVisible();
  await expect(page.getByText(/Telegram service is swapped for a local stand-in/i)).toBeVisible();

  const storage = await page.evaluate(() => ({
    localStorage: localStorage.length,
    cookies: document.cookie,
  }));
  expect(storage).toEqual({ localStorage: 0, cookies: "" });
  const origin = new URL(page.url()).origin;
  expect(requests.every((url) => new URL(url).origin === origin)).toBe(true);
  expect(requests.some((url) => /telegram|t\.me|api\.telegram/i.test(url))).toBe(false);
});

test("default WebGL renderer creates canvas", async ({ page }) => {
  await openPoster(page);
  await page.getByRole("button", { name: "Load interactive demo" }).click();
  await expect(page.locator("#plate")).toHaveAttribute("data-state", "running");
  await expect(page.frameLocator("#demo-frame").locator("canvas")).toBeVisible();
});
