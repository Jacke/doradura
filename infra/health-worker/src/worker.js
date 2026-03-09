// Cloudflare Worker — external health monitor for doradura bot.
//
// Runs every 1 minute via cron trigger. Pings the bot's public /health
// endpoint. Only calls Telegram API when state actually changes (to avoid
// rate limits — setMyName has a strict 429 limit).
//
// Uses getMe to read current name and compare before calling setMyName.
//
// Secrets (set via `wrangler secret put`):
//   BOT_TOKEN     — Telegram bot token

const HEALTH_URL = "https://doradura-production.up.railway.app/health";
const TELEGRAM_API = "https://api.telegram.org";

const ONLINE_NAME = "Dora \u2013 Downloader Youtube Instagram TikTok";
const OFFLINE_NAME = "Dora \u2013 Sleep";

export default {
  async scheduled(event, env, ctx) {
    await checkAndUpdate(env.BOT_TOKEN);
  },

  // Allow manual testing via HTTP request
  async fetch(request, env) {
    const result = await checkAndUpdate(env.BOT_TOKEN);
    return new Response(JSON.stringify(result, null, 2), {
      headers: { "Content-Type": "application/json" },
    });
  },
};

async function checkAndUpdate(token) {
  const healthy = await checkHealth();
  const desiredName = healthy ? ONLINE_NAME : OFFLINE_NAME;

  // Check current name first to avoid unnecessary API calls
  const currentName = await getCurrentName(token);

  if (currentName === desiredName) {
    console.log(`Health: ${healthy ? "UP" : "DOWN"} — name already correct, skipping`);
    return { healthy, name: desiredName, action: "none" };
  }

  // Name differs — update it
  console.log(`Health: ${healthy ? "UP" : "DOWN"} — changing name: "${currentName}" → "${desiredName}"`);
  const result = await setName(token, desiredName);

  if (!result.ok) {
    console.error(`setMyName failed: ${JSON.stringify(result)}`);
  }

  return { healthy, name: desiredName, previousName: currentName, action: "updated", telegram: result };
}

async function checkHealth() {
  try {
    const resp = await fetch(HEALTH_URL, {
      signal: AbortSignal.timeout(10000),
    });
    if (!resp.ok) return false;
    const body = await resp.text();
    return body.includes("healthy") || body.includes("ok");
  } catch {
    return false;
  }
}

async function getCurrentName(token) {
  try {
    const resp = await fetch(`${TELEGRAM_API}/bot${token}/getMe`);
    const data = await resp.json();
    return data.ok ? data.result.first_name : null;
  } catch {
    return null;
  }
}

async function setName(token, name) {
  try {
    const resp = await fetch(`${TELEGRAM_API}/bot${token}/setMyName`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    return await resp.json();
  } catch (e) {
    return { ok: false, error: e.message };
  }
}
