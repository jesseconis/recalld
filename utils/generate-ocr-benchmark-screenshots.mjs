import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";

import { chromium } from "playwright";
import toml from "toml";

const DEFAULT_VIEWPORT = { width: 1440, height: 1080 };
const DEFAULT_DELAY_MS = 1200;
const SELECTOR_CANDIDATES = ["main", "[role='main']", "article", "#content", "#main", "body"];

function parseArgs(argv) {
  const args = { manifest: "ocr-benchmark.toml" };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--manifest") {
      args.manifest = argv[index + 1];
      index += 1;
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      printHelp();
      process.exit(0);
    }
    throw new Error(`unknown argument: ${arg}`);
  }

  return args;
}

function printHelp() {
  console.log(`Usage: npm run ocr-benchmark:screens -- --manifest <path>`);
}

async function readManifest(manifestPath) {
  const text = await fs.readFile(manifestPath, "utf8");
  const manifest = toml.parse(text);
  if (!Array.isArray(manifest.cases) || manifest.cases.length === 0) {
    throw new Error(`manifest must contain at least one [[cases]] entry: ${manifestPath}`);
  }
  return manifest;
}

async function waitForPrimaryContent(page, caseSpec) {
  if (caseSpec.wait_for_selector) {
    const locator = page.locator(caseSpec.wait_for_selector).first();
    await locator.waitFor({ state: "visible", timeout: 15000 });
    return caseSpec.capture_selector ?? caseSpec.wait_for_selector;
  }

  if (caseSpec.capture_selector) {
    const locator = page.locator(caseSpec.capture_selector).first();
    await locator.waitFor({ state: "visible", timeout: 15000 });
    return caseSpec.capture_selector;
  }

  for (const selector of SELECTOR_CANDIDATES) {
    const locator = page.locator(selector).first();
    try {
      await locator.waitFor({ state: "visible", timeout: 1200 });
      return selector;
    } catch {
      // Try the next candidate.
    }
  }

  return null;
}

async function preparePage(page) {
  await page.addStyleTag({
    content: `
      *, *::before, *::after {
        animation-duration: 0s !important;
        animation-delay: 0s !important;
        transition-duration: 0s !important;
        scroll-behavior: auto !important;
      }
    `,
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.evaluate(async () => {
    if (document.fonts?.ready) {
      await document.fonts.ready;
    }
  });
}

async function captureCase(page, manifestDir, caseSpec) {
  if (!caseSpec.url) {
    console.log(`skip ${caseSpec.name}: no url field`);
    return;
  }
  if (!caseSpec.image) {
    throw new Error(`case ${caseSpec.name} is missing image`);
  }

  const screenshotPath = path.resolve(manifestDir, caseSpec.image);
	console.log(screenshotPath);
  await fs.mkdir(path.dirname(screenshotPath), { recursive: true });

  const viewport = {
    width: Number(caseSpec.viewport_width ?? DEFAULT_VIEWPORT.width),
    height: Number(caseSpec.viewport_height ?? DEFAULT_VIEWPORT.height),
  };
  await page.setViewportSize(viewport);

	// console.log(`capture ${caseSpec.name}: ${caseSpec.url}`);
  await page.goto(caseSpec.url, { waitUntil: "domcontentloaded", timeout: 30000 });
  await preparePage(page);

  const selector = await waitForPrimaryContent(page, caseSpec);
  const delayMs = Number(caseSpec.delay_ms ?? DEFAULT_DELAY_MS);
  if (delayMs > 0) {
    await page.waitForTimeout(delayMs);
  }

  if (caseSpec.full_page) {
    await page.screenshot({
      path: screenshotPath,
      fullPage: true,
      animations: "disabled",
    });
    return;
  }

  if (selector) {
    try {
      await page.locator(selector).first().evaluate((element) => {
        element.scrollIntoView({ block: "start", inline: "nearest" });
      });
      await page.waitForTimeout(150);
    } catch (error) {
      console.warn(`content positioning failed for ${caseSpec.name} (${selector}): ${error.message}`);
    }
  }

  await page.screenshot({
    path: screenshotPath,
    fullPage: false,
    animations: "disabled",
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const manifestPath = path.resolve(process.cwd(), args.manifest);
  const manifestDir = path.dirname(manifestPath);
  const manifest = await readManifest(manifestPath);

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    colorScheme: "light",
    deviceScaleFactor: 1,
    locale: "en-US",
  });
  const page = await context.newPage();

  let failures = 0;
  try {
    for (const caseSpec of manifest.cases) {
      try {
        await captureCase(page, manifestDir, caseSpec);
      } catch (error) {
        failures += 1;
        console.error(`failed ${caseSpec.name}: ${error.message}`);
      }
    }
  } finally {
    await context.close();
    await browser.close();
  }

  if (failures > 0) {
    process.exitCode = 1;
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
