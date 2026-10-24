// Page load of the Pictesa studio and another site under the same emulated connections. The local
// studio gets the measured round trip to the other site added, so both pay the same latency.
// Usage: npm i playwright-core && node scripts/pageload.mjs <studio-url> <extra-ms> <other-url>
import { chromium } from 'playwright-core';
import fs from 'node:fs';
// Chromium from PLAYWRIGHT_CHROMIUM, else the newest one installed by `npx playwright install chromium`.
const cache = process.env.HOME + '/.cache/ms-playwright';
const exe = process.env.PLAYWRIGHT_CHROMIUM ?? `${cache}/${fs.readdirSync(cache).filter((d) => d.startsWith('chromium-')).sort().at(-1)}/chrome-linux64/chrome`;
const [local, extra, remote] = process.argv.slice(2);
const profiles = [
  { name: 'broadband 100/20 Mbps +10 ms', down: 100e6, up: 20e6, latency: 10 },
  { name: 'typical 20/5 Mbps +40 ms', down: 20e6, up: 5e6, latency: 40 },
  { name: 'mobile 9/1.5 Mbps +100 ms', down: 9e6, up: 1.5e6, latency: 100 },
];
const browser = await chromium.launch({ executablePath: exe, args: ['--no-sandbox'] });
async function load(url, net) {
  const runs = [];
  for (let i = 0; i < 3; i++) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await ctx.newPage();
    const cdp = await ctx.newCDPSession(page);
    await cdp.send('Network.enable');
    await cdp.send('Network.setCacheDisabled', { cacheDisabled: true });
    await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: net.latency, downloadThroughput: net.down / 8, uploadThroughput: net.up / 8 });
    const t0 = Date.now();
    await page.goto(url, { waitUntil: 'load', timeout: 180000 }).catch(() => {});
    const loadMs = Date.now() - t0;
    await page.waitForTimeout(2500);
    const fcp = await page.evaluate(() => performance.getEntriesByType('paint').find((e) => e.name === 'first-contentful-paint')?.startTime ?? null);
    runs.push({ load: loadMs, fcp: fcp && Math.round(fcp) });
    await ctx.close();
  }
  const med = (k) => runs.map((r) => r[k]).sort((a, b) => a - b)[1];
  return { fcp: med('fcp'), load: med('load') };
}
for (const p of profiles) {
  const a = await load(local, { ...p, latency: p.latency + Number(extra) });
  const b = await load(remote, p);
  console.log(JSON.stringify({ profile: p.name, pictesa: a, higgsfield: b }));
}
await browser.close();
