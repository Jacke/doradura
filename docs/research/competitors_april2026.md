# Competitor Research — Telegram Download Bots & Tooling
## Status as of April 1, 2026

> Research conducted April 1, 2026. Sources: web search aggregation, status pages, GitHub releases, community forums.

---

## Table of Contents

1. [Established Competitors — Status Check](#1-established-competitors--status-check)
2. [New Entrants 2025–2026](#2-new-entrants-20252026)
3. [Cobalt.tools — Deep Dive](#3-cobalttools--deep-dive)
4. [yt-dlp — Engine Status](#4-yt-dlp--engine-status)
5. [Market Trends](#5-market-trends)
6. [Competitive Positioning for Doradura](#6-competitive-positioning-for-doradura)

---

## 1. Established Competitors — Status Check

### @YtbDownBot

| Field | Detail |
|-------|--------|
| **Status** | Alive, maintained |
| **Handle** | [@YtbDownBot](https://t.me/YtbDownBot) |
| **Maintainer** | @alexmboss (Ukrainian flag branding) |
| **Supported sites** | YouTube, TikTok, "most web sites" |
| **Formats** | Video (360p–1080p), MP3 audio, captions |
| **Languages** | English, Russian, Ukrainian |
| **Pricing** | Free |
| **Engine** | youtube-dl / yt-dlp (open-source GitHub repo: todokku/YtbDownBot is stale, but the running bot appears updated) |

**Assessment:** Functional but minimal feature set. No audio effects, no format selection beyond basic quality tiers, no ringtone, no lyrics. Positioned as a simple link-in → file-out bot. Competition is primarily on existence and simplicity, not feature depth. The GitHub source repo (todokku/YtbDownBot) was last meaningfully updated years ago, suggesting the live bot may be running a fork or reimplementation.

**Weaknesses for Doradura to exploit:** No advanced audio processing, no Spotify support, no batch operations, no TUI companion, audio quality likely capped below 320 kbps.

---

### @YTSaveBot (also @ytsavebot)

| Field | Detail |
|-------|--------|
| **Status** | Alive, ~513K monthly users |
| **Handle** | [@ytsavebot](https://t.me/ytsavebot) |
| **Supported sites** | YouTube, TikTok, Instagram, Pinterest |
| **Formats** | MP3, MP4 (114p to 480p max) |
| **Pricing** | Free |
| **Reliability** | Occasional downtime reported; maintenance windows common |

**Assessment:** High user count but a ceiling at 480p video quality is a notable gap. Covering YouTube, TikTok, and Instagram satisfies most casual users, which explains the large MAU. However the quality cap makes it unsuitable for anyone wanting 720p+ downloads. The user base skews toward mobile casual downloaders.

**Weaknesses for Doradura to exploit:** 480p video ceiling. No audio quality selection (320 kbps MP3 unavailable). Limited site coverage. No audio effects. No document mode. No video splitting for large files.

---

### @MusicsHuntersbot (also seen as @MusicsHunterBot)

| Field | Detail |
|-------|--------|
| **Status** | Alive, actively used |
| **Handle** | [@MusicsHuntersbot](https://t.me/MusicsHuntersbot) |
| **Supported sites** | YouTube, Spotify, Deezer, Qobuz, SoundCloud, and more |
| **Formats** | MP3 (320 kbps), FLAC |
| **Batch downloads** | Yes — up to 400 songs per session, delivered 9–10 at a time |
| **Metadata** | Displays track metadata before delivery |
| **Pricing** | Free |
| **Limitation** | Slows significantly on large playlists (400 songs = long wait) |

**Assessment:** The strongest audio-focused competitor. The 400-song batch capability is genuinely differentiated. FLAC support at Qobuz quality is notable and targets audiophiles. However the bot is audio-only (music), has no video download capability, and no in-Telegram processing features (no pitch, no tempo, no ringtone conversion).

**Weaknesses for Doradura to exploit:** No video. No audio effects/DSP. No ringtone creation flow. No TUI. The batch slowdown at large playlists is a user pain point.

---

### @SaveBot (general-purpose Telegram save/forward tool)

| Field | Detail |
|-------|--------|
| **Status** | Unclear / effectively dead for external URL downloads |
| **Handle** | Multiple bots use this or similar names; the original @SaveBot appears to no longer be the primary reference |
| **Note** | The "SaveBot" brand has fragmented — @SaveOFFbot, @allsaverbot, and @SaveVideoBot are the actively cited alternatives |

**Assessment:** The original @SaveBot concept (save restricted Telegram content) has been superseded. For external URL downloads, @allsaverbot and @SaveVideoBot are now the referenced alternatives. Any "SaveBot" brand recognition has largely dissolved into confusion across multiple similarly-named bots.

---

### @SaveVideoBot

| Field | Detail |
|-------|--------|
| **Status** | Most-used download bot as of February 2026 |
| **Supported formats** | Most video formats, files up to 2 GB |
| **Pricing** | Free |
| **Reliability** | Can be overloaded during peak hours; @TGDownloaderBot positioned as backup |
| **Note** | Some sources contradict — one states "no longer available," majority confirm it is still active |

**Assessment:** The de-facto market leader by volume. Broad format support and 2 GB file size handling are strong. However it functions as a generic forwarder/downloader without quality selection, audio-specific features, or any processing capabilities. Its popularity makes it a benchmark, not a feature competitor for Doradura.

---

### @allsaverbot

| Field | Detail |
|-------|--------|
| **Status** | Active |
| **Handle** | [@allsaverbot](https://t.me/allsaverbot) |
| **Supported sites** | Instagram, TikTok, Likee, Jio, YouTube, and others |
| **Special** | Also handles forwarded Telegram messages (restricted content) |
| **Pricing** | Free |

**Assessment:** A "Swiss Army knife" bot covering both external URL downloads and Telegram restricted-content forwarding. The dual use case differentiates it. Still lacks audio quality control, effects, or any processing pipeline.

---

## 2. New Entrants 2025–2026

### @YtbAudioBot

| Field | Detail |
|-------|--------|
| **Status** | Active (2026 list) |
| **Handle** | [@YtbAudioBot](https://t.me/YtbAudioBot) |
| **Focus** | YouTube audio only — songs and podcasts |
| **Quality** | 320 kbps MP3 |
| **Extra** | Track previews before download |
| **Pricing** | Free |

**Assessment:** Directly in Doradura's audio lane. Simpler than Doradura (no effects, no multi-site support), but the preview-before-download UX is a genuinely useful touch worth noting for the PRD.

---

### @scdlbot

| Field | Detail |
|-------|--------|
| **Status** | Active |
| **Handle** | [@scdlbot](https://t.me/scdlbot) |
| **Supported sites** | YouTube, SoundCloud, Bandcamp |
| **Formats** | MP3 with tags and artwork preserved |
| **Pricing** | Free |

**Assessment:** Niche music downloader with strong metadata/artwork handling. Not a broad competitor, but strong in the indie-music / SoundCloud segment. Bandcamp support differentiates it from most.

---

### @inst4youBot

| Field | Detail |
|-------|--------|
| **Status** | Active (2025–2026) |
| **Focus** | Instagram and TikTok |
| **Quality** | Original quality, watermark-free |
| **Pricing** | Free tier (daily limits + ads); Premium from ~$0.60/day up to ~$14 for 6 months, paid via Telegram Stars |
| **Note** | One of the few explicitly monetized bots via Telegram Stars premium tiers |

**Assessment:** Significant because it demonstrates that Telegram Stars-based freemium monetization is working in the downloader space. The pricing structure (short-term day passes up to 6-month plans) is a direct monetization template. Focused on social media only — not a YouTube-primary competitor.

---

### @AudioFMbot

| Field | Detail |
|-------|--------|
| **Status** | Active |
| **Focus** | Music finder + downloader, lyric retrieval |
| **Supported sites** | TikTok, Instagram, VK, music platforms |
| **Pricing** | Free |

**Assessment:** Combined music discovery + download + lyrics is an interesting bundle. Lyrics feature overlaps with Doradura's existing lyrics functionality.

---

### @YTBMusBot

| Field | Detail |
|-------|--------|
| **Status** | Active (timeout errors reported by users) |
| **Focus** | YouTube and YouTube Music MP3 |
| **Quality** | 320 kbps |
| **Extra** | Partial lyrics included; group chat support |
| **Pricing** | Free |

**Assessment:** Group chat support is a differentiator not currently featured prominently in Doradura. Timeout errors suggest infrastructure limitations common to single-person-maintained bots.

---

### @DownloadsMasterBot

| Field | Detail |
|-------|--------|
| **Status** | Active |
| **Handle** | [@DownloadsMasterBot](https://t.me/DownloadsMasterBot) |
| **Supported sites** | YouTube, Spotify, Instagram, TikTok, Threads, Twitter/X, Facebook, Twitch |
| **Pricing** | Unknown |

**Assessment:** Broad site coverage attempting to match Doradura's multi-site value proposition. No publicly documented audio effects, quality control, or processing pipeline. Likely yt-dlp wrapper without Rust-level performance.

---

## 3. Cobalt.tools — Deep Dive

### Current Status: Severely Degraded for YouTube

**cobalt.tools** is the most design-forward media downloader web tool in the space — ad-free, clean UI, open-source (self-hostable). As of April 1, 2026, its status is:

| Component | Status |
|-----------|--------|
| Web service | Operational |
| Load balancer | Operational |
| Processing nodes (Kityune, Blossom, Nachos, Sunny) | All DISRUPTED |
| YouTube av1 health check | FAILING (unresolved as of April 1, 2026 11:03 UTC) |
| YouTube vp9 health check | FAILING (unresolved) |

### Root Cause

YouTube implemented harsh network-level restrictions targeting server/datacenter IPs beginning mid-2025. The core issue per cobalt's own team statement (X, June 2025):

> "After countless attempts, cobalt has been unable to restore downloading from YouTube for longer than a few hours at a time."

As of August 2025, YouTube's strict network limits remained present. The team acknowledged needing "a completely different way of interacting with YouTube that doesn't involve a centralized proxy server." No resolution has been announced as of April 2026.

### Self-Hosted Workaround

Self-hosting cobalt with a residential proxy is technically possible and documented. Railway even offers a one-click cobalt deploy template. However:

- Self-hosted instances face the same IP blocking unless routed through residential proxies
- YouTube's SABR (Server-Based Adaptive Bit Rate) protocol adds a challenge layer
- cobalt uses its own signature deciphering (not yt-dlp), which stales between YouTube player updates

### Strategic Implication for Doradura

Cobalt's YouTube failure is a **direct strategic opening**. Cobalt had strong brand recognition among technical users and design-conscious audiences. Those users are now displaced. Doradura's use of WARP proxy + PO token via bgutil + cookies fallback directly addresses the exact infrastructure problem cobalt failed to solve.

The Doradura architecture (proxy-routed yt-dlp with bgutil PO token server) is currently the **correct technical answer** to the problem cobalt could not solve.

---

## 4. yt-dlp — Engine Status

### Latest Stable Release

| Version | Date | Key Changes |
|---------|------|-------------|
| **2026.03.17** | March 17, 2026 | YouTube extractor fixes: `webpage_client` arg respected; `--live-from-start` fix; ejs updated to 0.8.0 |
| 2026.03.13 | March 13, 2026 | TikTok challenge fix; YouTube `android_vr` player fix; `web_embedded` client fix |
| 2026.03.03 | March 3, 2026 | YouTube player update forced; `webpage` player response skipped by default |
| 2026.02.21 | Feb 21, 2026 | ejs 0.5.0 — fixes YouTube sig extraction in main player variant |
| 2026.02.04 | Feb 4, 2026 | Deno runtime introduced for JavaScript challenge solving |

**Latest confirmed version: 2026.03.29** (per free-codecs.com listing as of April 2026)

### YouTube Bot Detection — Current Landscape

The situation as of April 2026 is significantly harder than 2024:

**SABR (Server-Based Adaptive Bit Rate):**
- YouTube is progressively rolling out SABR, a custom streaming protocol that replaces classic progressive/DASH downloads
- SABR requires a "handshake" that yt-dlp cannot perform on standard clients
- The `web` client now returns SABR-only formats in many regions (GitHub issue #12482)
- Workaround: use `android_vr`, `web_safari`, `web_embedded`, or `ios` player clients that still return non-SABR URLs

**IP Blocking — Cloud vs. Residential:**
- Datacenter/cloud IPs (AWS, Railway, Azure, GCP) achieve only 20–40% success rate against YouTube
- Residential and ISP proxies achieve 85–95% success rate
- Cloudflare WARP is specifically noted as an effective workaround because YouTube has not broadly flagged Cloudflare WARP IP ranges
- This validates Doradura's existing WARP_PROXY architecture as the correct production solution

**PO Token / bgutil:**
- bgutil-ytdlp-pot-provider v1.3.1 released March 7, 2026
- Deno >= 2.0.0 or Node.js >= 20 required for challenge solving
- Known issue: Deno can become stuck inside Docker containers (GitHub issue #16148) — this is a live production concern for containerized deployments like Doradura on Railway
- HTTP server mode (the bgutil sidecar approach) is recommended over the on-demand script mode

**JavaScript Runtime (Deno) Requirement:**
- yt-dlp 2026.02.04+ uses Deno for YouTube challenge solving (ejs library)
- ejs 0.8.0 (March 17) is the latest — fixes sig extraction reliability
- Alpine Linux requires `apk add deno --repository=.../alpine/edge/testing` for musl build

### Threat Assessment

yt-dlp is under active development with releases roughly every 2 weeks. The project responds to YouTube's countermeasures within days to weeks. The primary ongoing threats are:

1. **SABR rollout completion** — if YouTube enforces SABR for all clients, only clients that can perform the SABR handshake will work. yt-dlp team is tracking this (issue #12482).
2. **android_vr deprecation** — this client currently bypasses many restrictions; YouTube may deprecate it.
3. **Deno/Node.js Docker instability** — challenge solver hangs in containerized environments remain an unresolved reliability issue.

---

## 5. Market Trends

### Trend 1: Fragmentation and Consolidation Simultaneously

The market is simultaneously fragmenting (dozens of new small bots for specific niches: ringtones, shorts, music only) and consolidating around a small set of reliable names (@SaveVideoBot, @MusicsHuntersbot). Users cycle between bots as reliability fluctuates. There is no dominant all-in-one bot with strong brand recognition and consistent uptime.

### Trend 2: Monetization via Telegram Stars Is Proven

@inst4youBot demonstrates that downloader bots can successfully monetize via Telegram Stars with tiered pricing ($0.60/day to $14/6 months). This validates Doradura's subscription model. The market accepts payment for reliability and premium access.

### Trend 3: YouTube's War on Downloaders Is Escalating

Between SABR protocol rollout, PO token requirements, and aggressive IP blocking of datacenter ranges, YouTube has entered a sustained campaign against automated downloading. Services that cannot route through residential IPs (cobalt.tools, many simple bots) are failing. Only technically sophisticated operators with proxy infrastructure survive. This creates a **significant moat** for operators who have solved the infrastructure problem.

### Trend 4: "No-Registration, Works Immediately" Remains the Key UX Expectation

All successful bots emphasize zero friction: paste a link, get a file. Any bot requiring account creation, login, or external redirect loses users immediately. Telegram as a platform amplifies this — the bot interaction IS the UI.

### Trend 5: Multi-Platform Coverage Is Table Stakes; Processing Is the Differentiator

By 2026, supporting YouTube + Instagram + TikTok is considered baseline. The bots that stand out are those adding a processing layer: audio effects, format conversion, ringtone generation, subtitle burning, video splitting. Doradura's feature set (pitch, tempo, bass, lofi, wide, morph, ringtone, SRT, voice message) is above market in this dimension.

### Trend 6: Reliability > Features for Retention

Users' primary complaint across all bots is downtime and timeouts. A bot that always works at lower quality beats a bot that sometimes works at higher quality. Infrastructure reliability is the core product in this market.

---

## 6. Competitive Positioning for Doradura

### Competitive Matrix (April 2026)

| Feature | Doradura | YtbDownBot | YTSaveBot | MusicsHuntersbot | SaveVideoBot | cobalt.tools |
|---------|---------|------------|-----------|-----------------|--------------|-------------|
| **YouTube works (cloud)** | Yes (WARP) | Unknown | Degraded | Partial | Partial | NO |
| **1000+ sites** | Yes | Partial | No (~4) | Partial | Yes | Yes (when working) |
| **Max video quality** | 1080p+ | 1080p | 480p | N/A (audio) | Any | 1080p (broken) |
| **Audio quality** | 320 kbps | Unknown | Unknown | 320 kbps / FLAC | Unknown | Varies |
| **Audio effects** | Yes (6 types) | No | No | No | No | No |
| **Ringtone creation** | Yes (iPhone+Android) | No | No | No | No | No |
| **Lyrics** | Yes | No | No | No | No | No |
| **Batch / playlist** | Partial | No | No | Yes (400 songs) | No | No |
| **Subtitle burning** | Yes | No | No | No | No | No |
| **File size limit** | 2 GB (local Bot API) | 50 MB (cloud) | 50 MB (cloud) | 50 MB (cloud) | 2 GB (local Bot API) | 2 GB |
| **TUI client** | Yes (dora) | No | No | No | No | No |
| **Monetized** | Yes (Telegram Stars) | No | No | No | No | API (self-host) |
| **Open source** | Closed | Partially | No | No | No | Yes |
| **Reliability** | High (WARP + bgutil) | Medium | Medium | Medium | Medium | LOW (YouTube broken) |

### Doradura's Defensible Moat

1. **Infrastructure** — WARP proxy routing solves the cloud IP blocking problem that killed cobalt.tools. This is not easy to replicate.
2. **Processing pipeline** — Rust + yt-dlp + ffmpeg with audio effects, ringtone conversion, subtitle burning is feature depth no competitor matches.
3. **File size** — Local Bot API enables 2 GB files; cloud-API bots are capped at 50 MB, which breaks most HD video downloads.
4. **Dual product** — TUI companion (dora) serves power users and creates a distinct product category competitors have not entered.

### Opportunities Identified

1. **Cobalt refugee users** — cobalt's YouTube failure displaced a design-conscious, technically-savvy user segment that would respond well to Doradura's quality positioning.
2. **Batch playlist downloads** — @MusicsHuntersbot owns this segment but is audio-only. A playlist download feature for Doradura (even limited to 10–20 tracks) would capture music-primary users.
3. **Group chat mode** — @YTBMusBot differentiates on group support; Doradura does not currently advertise this.
4. **Preview before download** — @YtbAudioBot's preview UX is a small but valued quality-of-life feature worth evaluating.
5. **FLAC output** — @MusicsHuntersbot offers FLAC; Doradura currently tops out at 320 kbps MP3. Adding FLAC as a premium format tier would directly compete in the audiophile segment.

---

## Sources

- [NoteBurner — Best Telegram Bots to Download YouTube to MP3 (2026 List)](https://www.noteburner.com/youtube-music-tips/youtube-to-mp3-telegram-bot.html)
- [cobalt status page](https://status.cobalt.tools/)
- [cobalt YouTube issues stub](https://status.cobalt.tools/issues/manual/2023-12-25-youtube-stub/)
- [cobalt on X — YouTube disruption announcement](https://x.com/justusecobalt/status/1935757615793160532)
- [yt-dlp 2026.03.17 release](https://github.com/yt-dlp/yt-dlp/releases/tag/2026.03.17)
- [yt-dlp 2026.03.13 release](https://github.com/yt-dlp/yt-dlp/releases/tag/2026.03.13)
- [yt-dlp 2026.03.03 release](https://github.com/yt-dlp/yt-dlp/releases/tag/2026.03.03)
- [bgutil-ytdlp-pot-provider PyPI](https://pypi.org/project/bgutil-ytdlp-pot-provider/)
- [bgutil-ytdlp-pot-provider GitHub](https://github.com/Brainicism/bgutil-ytdlp-pot-provider)
- [yt-dlp PO Token Guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide)
- [YouTube PO Token / JS runtime announcement — issue #15012](https://github.com/yt-dlp/yt-dlp/issues/15012)
- [SABR issue — yt-dlp #12482](https://github.com/yt-dlp/yt-dlp/issues/12482)
- [Deno stuck in Docker — yt-dlp issue #16148](https://github.com/yt-dlp/yt-dlp/issues/16148)
- [Leveraging Cloudflare WARP to bypass YouTube API restrictions](https://blog.arfevrier.fr/leveraging-cloudflare-warp-to-bypass-youtubes-api-restrictions/)
- [YouTube IP blocking discussion — Hacker News](https://news.ycombinator.com/item?id=43398222)
- [Bypassing 2026 YouTube Great Wall — DEV Community](https://dev.to/ali_ibrahim/bypassing-the-2026-youtube-great-wall-a-guide-to-yt-dlp-v2rayng-and-sabr-blocks-1dk8)
- [Telegram Video Downloader — ScreenApp (2026)](https://screenapp.io/blog/telegram-video-downloader)
- [Alphr — Telegram YouTube Downloaders](https://www.alphr.com/telegram-youtube-downloader/)
- [yt-dlp VideoHelp version history](https://www.videohelp.com/software/yt-dlp/version-history)
- [VRChat feature request — yt-dlp 2026.02.21](https://feedback.vrchat.com/feature-requests/p/update-yt-dlp-to-20260221-to-resolve-youtube-playback-issues)
- [cobalt.tools alternatives — Wondershare](https://videoconverter.wondershare.com/video-converters/cobalt-tools-alternative.html)
- [Deploy Cobalt Tools on Railway](https://railway.com/deploy/cobalt)
- [YouTube proxy guide — proxy001](https://proxy001.com/blog/youtube-proxy-prevent-server-ip-blocks-after-deploying-yt-dlp-style-server-workloads)
- [inst4youBot — Instagram & TikTok downloader](https://inst4youbot.com/)
- [Telegram bots list — findmini.app YtbAudioBot](https://www.findmini.app/ytbaudiobot/)
- [Telegramic — allsaverbot profile](https://telegramic.org/bot/allsaverbot/)
- [Telegramic — YtbDownBot profile](https://telegramic.org/bot/ytbdownbot/)
