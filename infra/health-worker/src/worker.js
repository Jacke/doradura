// Cloudflare Worker — external health monitor for doradura bot.
//
// Runs every 1 minute via cron trigger. Pings the bot's public /health
// endpoint. On state change, updates bot name AND avatar via Telegram API.
//
// Rate limit strategy:
//   - getMe (read current name) — no known rate limit
//   - setMyName — very strict (~few calls/day), only call on actual change
//   - setMyProfilePhoto — very strict, only call on actual change
//   Both are guarded by checking current state first.
//
// Secrets (set via `wrangler secret put`):
//   BOT_TOKEN — Telegram bot token

import ONLINE_AVATAR from "../assets/online.png";
import OFFLINE_AVATAR from "../assets/offline.png";

const HEALTH_URL = "https://doradura-production.up.railway.app/health";
const TELEGRAM_API = "https://api.telegram.org";

const ONLINE_NAME = "Dora \u2013 Downloader Youtube Instagram TikTok";
const OFFLINE_NAME = "Dora \u2013 Sleep";

export default {
  async scheduled(event, env, ctx) {
    await checkAndUpdate(env);
  },

  // Manual testing via HTTP request
  async fetch(request, env) {
    const result = await checkAndUpdate(env);
    return new Response(JSON.stringify(result, null, 2), {
      headers: { "Content-Type": "application/json" },
    });
  },
};

async function checkAndUpdate(env) {
  const token = env.BOT_TOKEN;
  if (!token) return { error: "BOT_TOKEN not set" };
  const healthy = await checkHealth();
  const desiredName = healthy ? ONLINE_NAME : OFFLINE_NAME;

  // Read current state
  const currentName = await getCurrentName(token);
  const nameCorrect = currentName === desiredName;

  if (nameCorrect) {
    console.log(`Health: ${healthy ? "UP" : "DOWN"} — status already correct, skipping`);
    return { healthy, name: desiredName, action: "none" };
  }

  // State changed — update name and avatar
  console.log(`Health: ${healthy ? "UP" : "DOWN"} — changing: "${currentName}" → "${desiredName}"`);

  const nameResult = await setName(token, desiredName);
  if (!nameResult.ok) {
    console.error(`setMyName failed: ${JSON.stringify(nameResult)}`);
  }

  const avatarData = healthy ? ONLINE_AVATAR : OFFLINE_AVATAR;
  const avatarResult = await setAvatar(token, avatarData);
  if (!avatarResult.ok) {
    console.error(`setMyProfilePhoto failed: ${JSON.stringify(avatarResult)}`);
  }

  return {
    healthy,
    name: desiredName,
    previousName: currentName,
    action: "updated",
    nameResult,
    avatarResult,
  };
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

// Bot API 9.4: setMyProfilePhoto expects InputProfilePhotoStatic JSON
// with the file sent as a separate multipart part via attach:// reference.
async function setAvatar(token, pngData) {
  try {
    const form = new FormData();
    form.append("photo", JSON.stringify({ type: "static", photo: "attach://photo_file" }));
    form.append("photo_file", new Blob([pngData], { type: "image/png" }), "photo.png");

    const resp = await fetch(`${TELEGRAM_API}/bot${token}/setMyProfilePhoto`, {
      method: "POST",
      body: form,
    });
    return await resp.json();
  } catch (e) {
    return { ok: false, error: e.message };
  }
}
