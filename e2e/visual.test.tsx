import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect, open, test } from "nsh/test";

const SHOT_DIR = process.env.SHOT_DIR ?? "/tmp/scratchrunner-shots";

type ScreenshotTarget = {
  screenshot(path?: string | null): Promise<Uint8Array>;
};

async function snap(target: ScreenshotTarget, name: string): Promise<void> {
  try {
    await target.screenshot(`${SHOT_DIR}/${name}.png`);
  } catch (err: unknown) {
    const message = String(err);
    if (
      message.toLowerCase().includes("headless") ||
      message.includes("a screenshot needs a real platform") ||
      message.includes("ERR_NSH_TEST_UNSUPPORTED")
    ) {
      return;
    }
    throw err;
  }
}

test(
  "visual parity audit in light and dark modes",
  async () => {
    fs.mkdirSync(SHOT_DIR, { recursive: true });
    process.env.SCRATCHRUNNER_DATA_DIR ??= fs.mkdtempSync(
      path.join(os.tmpdir(), "scratchrunner-e2e-"),
    );
    const app = await open("..", {
      width: 1280,
      height: 820,
      platform: "macos",
      colorScheme: "light",
    });

    await app.waitFor("Syntax: Shell Script", { timeout: 10_000 });
    await app.waitFor("Line 2, Column 33", { timeout: 5_000 });
    expect(app.shows("Tabs: 4")).toBe(true);

    // Run Tab 1's welcome Shell script first and open Console so output renders.
    await app.keyboard.press("mod+r");
    await app.waitFor("text:/Run Succeeded/", { timeout: 10_000 });
    if (app.shows("Show Console")) {
      await app.press("Show Console");
    }
    await app.waitFor("label:Console output filter", { timeout: 5_000 });
    await app.waitFor("Welcome to ScratchRunner!", { timeout: 5_000 });
    await app.wait(300);

    // Open a second tab (`Untitled 2.sh`) so the tab strip & multi-tab visual states
    // are exercised, then return to Tab 1 (mod+1) so the welcome script and its console remain visible.
    await app.keyboard.press("mod+t");
    await app.waitFor("Untitled 2.sh", { timeout: 5_000 });
    await app.keyboard.press("mod+1");
    await app.waitFor("text:/Run Succeeded/", { timeout: 5_000 });
    if (app.shows("Show Console")) {
      await app.press("Show Console");
    }
    await app.waitFor("Welcome to ScratchRunner!", { timeout: 5_000 });

    // Ensure the File Navigator sidebar is open so screenshots show the 3-column workbench.
    if (!app.shows("label:Search")) {
      await app.keyboard.press("cmd+shift+h");
      await app.waitFor("label:Search", { timeout: 5_000 });
    }

    for (const scheme of ["light", "dark"] as const) {
      await app.setColorScheme(scheme);
      await app.wait(350);

      // 1. Main workbench (Navigator + Editor + live Console + 24pt Status Bar).
      await snap(app, `01-workbench-${scheme}`);

      // 2. Run Settings popover over toolbar.
      await app.keyboard.press("mod+shift+r");
      await app.waitFor("Arguments:", { timeout: 5_000 });
      await app.wait(300);
      await snap(app, `02-run-settings-${scheme}`);
      await app.press("Done");
      await app.waitFor("Program Input:", { state: "gone", timeout: 5_000 });

      // 3. Documentation right panel (source strip + 24pt Contents TOC footer).
      await app.keyboard.press("mod+shift+i");
      await app.waitFor("Contents", { timeout: 5_000 });
      await app.wait(300);
      await snap(app, `03-docs-panel-${scheme}`);
      await app.keyboard.press("mod+shift+i");
      await app.waitFor("Contents", { state: "gone", timeout: 5_000 });

      // 4. Find & Replace bar + Project Search sidebar.
      await app.keyboard.press("mod+alt+f");
      await app.waitFor("All In Selection", { timeout: 5_000 });
      await app.press("label:Search");
      await app.waitFor("File Contents + Filenames", { timeout: 5_000 });
      await app.hover("text:Untitled.sh >> nth=0");
      await app.wait(250);
      await snap(app, `04-find-and-search-${scheme}`);
      await app.press("label:Hide the search view");
      await app.waitFor("label:Search", { timeout: 5_000 });
      await app.press("Done");
      await app.waitFor("All In Selection", { state: "gone", timeout: 5_000 });
      await app.wait(200);

      // 5. Preferences window across all panes (General, Text Editing, Themes, Languages, Keyboard, Shell, Advanced).
      await app.keyboard.press("mod+,");
      const prefs = app.window("prefs");
      await app.waitUntil(() => prefs.state()?.visible === true, { timeout: 5_000 });
      await app.wait(300);
      await snap(prefs, `05-prefs-default-${scheme}`);

      for (const [slug, label] of [
        ["text-editing", "Text Editing"],
        ["themes", "Themes"],
        ["languages", "Languages"],
        ["keyboard", "Keyboard"],
        ["shell", "Shell"],
        ["advanced", "Advanced"],
      ] as const) {
        if (app.shows(`label:${label}`)) {
          await app.press(`label:${label}`);
          await app.wait(350);
          await snap(prefs, `05-prefs-${slug}-${scheme}`);
        }
      }
      await prefs.pressClose();
      await app.wait(250);

      // 6. Native ScratchRunner Help window (triggered with main window focused).
      await app.keyboard.press("mod+shift+/");
      const help = app.window("help");
      await app.waitUntil(() => help.state()?.visible === true, { timeout: 5_000 });
      await app.wait(350);
      await snap(help, `06-help-window-${scheme}`);
      await help.pressClose();
      await app.wait(250);
    }
  },
  120_000,
);
