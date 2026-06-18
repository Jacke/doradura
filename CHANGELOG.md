# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

> 🎉 **Pre-release track promoted alpha → beta (v0.51.0-beta.1, 2026-06-07).** Core feature set (inline mode + identity guard, silent downloads, Instagram Stories, period/smart history search, popular_files viral cache) считается feature-complete и достаточно стабильным для бета-тестинга. Дальше нумерация `0.51.0-beta.N` вместо `alpha.N`. Историю alpha.X-меток в записях ниже оставляем как есть — это хронология появления фич.

### Changed
- **Stories: посегментный параллельный энкод (concurrency) + точные длительности** (v0.51.0-beta.25) — рендер Stories больше не один последовательный ffmpeg-проход. Теперь каждый сегмент кодируется **независимым ffmpeg параллельно** (`encode_segments_parallel`: `-ss s*seg -t seg` на сегмент, bounded `Semaphore` по vCPU из `available_parallelism`, `JoinSet`) → клип на N сегментов жмёт до N сегментов разом вместо простаивающих ядер. **Бонус-фикс:** длительности сегментов теперь **точные** (15/15/15/15) — старый single-pass `-c copy`/segment-muxer резал по keyframe и давал кривые (23/15/15/6). Пресеты ускорены (IG всё равно перекодирует): Std `medium→veryfast` crf 20→23, Max `slow→fast` crf 18→20. `Original` AR (stream-copy) и неизвестная длительность → single-pass фолбэк. Бенч на Railway (24 vCPU): 60с→4×15с **23с→14с** + точные сегменты + A/V ок. Концуррентность масштабируется с числом сегментов (мелкие сегменты = быстрее); одиночный 60с-сегмент выигрывает от пресета.

### Fixed
- **Реки: разделены кэш-ключи For You / Похожее (баг при двух rec-сообщениях)** (v0.51.0-beta.24) — `✨ Для тебя` и `🎧 Похожее` делили один prompt_session-ключ, поэтому при двух rec-сообщениях в чате тап по номеру в старом сообщении резолвил трек из последнего списка (не того). Теперь раздельные ключи (`foryou_recs` / `similar_recs`) + тег в callback `exp:rec:{f|s}:{idx}` → тап всегда резолвится против своего списка. Также: `.specify/companion.yml` (случайно попал в beta.22) убран из репо + в `.gitignore`.

### Added
- **🎧 «Похожее» — реки по конкретному видео из превью** (v0.51.0-beta.23) — на превью-карточке YouTube-видео кнопка «🎧 Похожее» (`exp:like:{url_id}`): резолвит URL → `recommend::similar_to` (YouTube Mix `RD<id>` минус сам сид) → присылает отдельное сообщение с rich-карточками похожих; тап по номеру → превью-карточка (через тот же `exp:rec:{idx}`, кэш в prompt_session). Это «лови такие видео» по конкретной ссылке (в дополнение к персональной вкладке ✨ Для тебя). Кнопка добавляется только для YouTube-превью (оба preview-сендера), не трогая builder'ы (дописывается в `send_preview`, где есть `lang`/`url_id`/`is_youtube`). Новый `render_recommendations_keyboard_plain` (номерные кнопки без таб-бара). i18n ×4. Аддитивно.

### Added
- **Движок рекомендаций + вкладка ✨ «Для тебя» в Explore** (v0.51.0-beta.22) — «ловит такие видео»: персональные музыкальные реки по вкусу. Движок: `doracore::recommend` (чистое ранжирование — `rank` по cross-seed частоте + `blend_fill`, 4 теста) + `dorabot::download::recommend` (`radio_for` — тянет **YouTube Mix/Radio `RD<id>`** через yt-dlp+cookies = родной алгоритм «похожих»; `recommend_for_user` — сиды = последние скачивания юзера → агрегирует миксы → дедуп → исключает уже скачанное → бленд глобального тренда `popular_files` для cold-start). Новая вкладка **✨ Для тебя** (`exp:tab:foryou`) рядом с Лента/Тренды/Подписки: считает реки on-demand (тост+loading), кэширует список в prompt_session, рендерит rich-карточки; тап по номеру (`exp:rec:{idx}`) → **превью-карточка** этого видео (уважает правило «превью всегда перед скачиванием», не one-tap). Проверено вживую: сальса-сет → Grupo Niche/Eddie Santiago; Rick Astley → Sweet Dreams/a-ha. i18n ×4. Аддитивно (не меняет download-args). Follow-up: кнопка «🎧 Похожее» на результате + проактивный пуш.

### Added
- **Кросс-платформа, Phase 0 шаг 1: нейтральный messaging-слой** (v0.51.0-beta.21) — фундамент под выход за пределы Telegram (WhatsApp/iMessage/X). Новый teloxide-free `doracore::messaging`: нейтральные типы (`Platform`, `ChatRef`/`UserRef`, `OutboundMessage` Text/Media, `MediaSource` LocalPath/Url/CachedRef, `Keyboard`/`Button` с `reflow`/`flat`, `MessageHandle`, `InboundEvent`/`InboundMessage`, `TextStyle`), `Capabilities` (per-platform: inline_keyboard, edit_in_place, max-кнопки, list_menu, text_style, media-лимиты, proactive_anytime — заданы для Telegram/WhatsApp/iMessage) + traits `Messenger` (send/edit/delete) и `InboundSource` (normalize webhook→neutral). 9 unit-тестов. **Дормантно** — пока никем не используется; следующий шаг (отдельный, осторожный) — Telegram-адаптер реализует `Messenger` и core send-путь (progress/send/pipeline) идёт через него (gated полным Telegram-регрессом). План: `~/.claude/plans/rosy-yawning-lemon.md`.

### Fixed
- **Скачивание Spotify-треков (и Apple Music/Deezer) — резолв в YouTube** (v0.51.0-beta.20) — прод-алерт: `open.spotify.com/track/…` падал «Failed to download video» 3× — yt-dlp не умеет Spotify (DRM, нет прямого аудио), а `source/ytdlp.rs` отдавал spotify в yt-dlp. Фикс: на интейке (`handle_message`, оба пути) одиночные track-URL'ы стриминг-платформ резолвятся в YouTube **до** скачивания. Новый `odesli::fetch_track(url) → {youtube, title, artist}` (song.link): (1) если есть прямой YouTube/YouTube-Music линк — берём его; (2) иначе fallback — YouTube-поиск по `artist + title` (многие треки без YT-записи в song.link находятся по названию; напр. упавший «Agatha (Calavera & Manya Remix)» на song.link имеет только Deezer). Покрывает spotify track/`spotify:track:`/Apple Music/Deezer track. Spotify playlist/album уже обрабатываются как импорт. Превью и скачивание идут по резолвнутому YouTube-URL.

### Fixed
- **`lyrics_overrides` не создавалась на SQLite-проде (миграция V50 не применялась)** (v0.51.0-beta.19) — пост-деплой проверка показала: таблицы `lyrics_overrides` НЕТ, хотя лог refinery писал «V50 Applied». Причина: на проде refinery откатывает **всю** пачку миграций при «duplicate column» от старых ALTER (история застряла на V18 ещё с давних пор), поэтому `.sql`-файлы V19–V50 на SQLite фактически не выполняются — реальная SQLite-схема новых таблиц живёт в коде `storage/migrations.rs::ensure_tables()` (popular_files/silent_digest/… создаются там идемпотентно). `lyrics_overrides` был только в V50.sql + pg_bootstrap, но не в `ensure_tables` → на SQLite не создавался (фича деградировала молча: override никогда не сохранялся/не применялся). Фикс: добавлен `CREATE TABLE IF NOT EXISTS lyrics_overrides` в `ensure_tables`. (Колонка `audio_effect_sessions.source_url` создавалась корректно — она идёт через code-ALTER `ensure_*`, проверено на проде: `source_url: YES`.)

### Added
- **Lyrics-коррекция при скачивании — шаг B: проверка override + ввод правильной ссылки** (v0.51.0-beta.18) — бот-обвязка поверх шага A. (1) `audio_effect_sessions.source_url` (новая колонка: SQLite через idempotent ALTER в коде, Postgres через bootstrap + `ADD COLUMN IF NOT EXISTS`) — source URL трека пробрасывается при создании сессии (`add_audio_effects_button` ← download_and_send_audio). (2) При запросе lyrics сначала проверяется **override по canonical source URL** — если есть, показывается сохранённый снапшот (минуя авто-матч). (3) В пикере/полном тексте/«не найдено» — кнопка **«❌ Wrong song / fix lyrics»** → армит prompt → юзер шлёт Genius/lrclib-ссылку → `providers::fetch_from_url` → сохраняется глобальный override (ключ = `canonicalize_url(source)`, last-wins) → показываются исправленные lyrics. Перехват ввода — новый `try_intercept_lyrics_fix` в цепочке `commands` (рядом с feedback/import). Всё на существующей lyrics-инфре (`fetch_lyrics_smart`/`parse_sections`/section-picker) + prompt_session. 662 doracore-теста зелёные.

### Added
- **Lyrics overrides — шаг A: провайдеры + каноническая таблица коррекций** (v0.51.0-beta.17) — фундамент под коррекцию lyrics при скачивании. (1) `doracore::lyrics::providers` — явный реестр источников (`LyricsProviderKind` lrclib/genius) + **`fetch_from_url(url)`**: резолвит lyrics из конкретной ссылки, которую дал юзер (Genius song-page скрейп / lrclib `api/get/{id}`), переиспользуя `parse_genius_html`/`parse_sections` родительского модуля; best-effort artist/title из `<title>`. (2) Миграция **V50 `lyrics_overrides`** (SQLite + Postgres bootstrap) — глобальная (каноническая) таблица: ключ = `canonicalize_url(source)`, снапшот текста + провайдер + source-link + `corrected_by`. (3) Storage-аксессоры `get_lyrics_override`/`upsert_lyrics_override` (db + shared, upsert last-wins). 5 unit-тестов (детект провайдера, lrclib id, парс genius-title, roundtrip, upsert). Дормантно — бот-обвязка (проверка override при fetch + кнопка «исправить» → ввод ссылки) идёт шагом B. Дизайн подтверждён: глобально / ввод ссылкой / снапшот.

### Removed
- **Откат дублирующего `doracore::playlist` (был beta.15)** (v0.51.0-beta.16) — beta.15 добавлял новый playlist-слой (trait `PlaylistProvider` + Spotify-провайдер), не заметив, что в `dorabot::download::playlist_sync` это **уже есть и полнее**: резолверы Spotify/SoundCloud/YandexMusic/YouTube + `PlaylistResolver` trait + `import_playlist` + таблицы `synced_playlists`/`synced_tracks` + меню-интеграция (`playlist_integrations.rs`). Дубликат удалён (CLAUDE.md: убирать дубликаты). Реальный недостающий кусок ⭐ Подписок — **периодический ре-синк подписок + проактивный пинг о новых треках** (watcher есть только для Instagram) — будет построен на существующем `playlist_sync` + `synced_playlists`.

### Added
- **Stories Wizard (Фаза B) — превью всех aspect ratio сеткой** (v0.51.0-beta.14) — на config-карточке Stories кнопка «👁 Превью всех форматов» (`downloads:stories:wiz:{id}`): бот качает исходник, берёт **один кадр из середины** клипа, центр-кропит его под каждый AR (9:16/1:1/4:5/16:9 + оригинал) и шлёт **альбомом** (Telegram рендерит сеткой — «примеры сразу во всех AR») + ряд кнопок выбора. Тап по AR (`wpick`) → открывает config-карточку с этим соотношением. **Только image-операции** (1 extract кадра + N кропов, без видео-энкодов) → дёшево и bounded, нет runaway-риска (cap 90с на ffmpeg, temp-dir с RAII-guard). ffmpeg extract+crop **проверены на Railway** перед коммитом (CLAUDE.md). Самодостаточно в `stories.rs` (`start_wizard`/`run_wizard`/`run_ffmpeg_quiet`). i18n ×4 (`stories-preview-*`, `stories-pick-ar`). **Отложено в follow-up:** сэмпл-клип на выбранный AR (сейчас сетка из стоп-кадров) и сохранение дефолтного AR per-user (нужна миграция).

### Added
- **Explore «🔥 Тренды» — настоящая вкладка (sub-project C)** (v0.51.0-beta.13) — таб Тренды больше не заглушка «🔜 Скоро»: показывает глобальный топ скачиваний из `popular_files` rich-карточками (ранг 1️⃣…🔟, тип+платформа эмодзи, `<b>артист</b> — title`, моно-бейдж `ФОРМАТ · размер · длительность`, **🔥 N скачали**). Тап по номеру → мгновенный resend файла из кешированного `file_id` (callback `exp:trs:{rank}` → re-fetch топа → отправка по типу). Новый аксессор `top_popular_files(limit)` (SQLite+Postgres, `ORDER BY hits DESC, last_used DESC LIMIT`), без миграций (таблица V48 уже есть). Рендер `render_trending_text/keyboard` переиспользует rich-хелперы. i18n ×4 (`explore_trending_empty`, `explore_hits`). 3 unit-теста (ordering, trending cards, empty). Подписки (sub-project B) пока остаются «🔜 Скоро».

### Changed
- **Карточка результата скачивания — rich tech-бейдж в подписи** (v0.51.0-beta.12) — caption отправленного файла теперь, кроме `*Артист* — _Title_`, несёт моноширинную строку-бейдж `` `ФОРМАТ · качество · платформа` `` (напр. `` `MP4 · 1080p · ▶️ YouTube` ``); главы (chapters) сохранены ниже. Новая `doracore::core::utils::format_media_caption_rich(title, artist, badge, timestamps)` (MarkdownV2; бейдж в code-span, ` и \ из бейджа вырезаются — не сломать парсинг). Бейдж строится в `pipeline.rs::download_phase` (формат из `format.label()`, качество из `pipeline_video_quality`, платформа из URL через `metrics::extract_platform` → новый `caption_platform`); т.к. `execute` (audio) переиспользует `download_phase`, бейдж получают **и аудио, и видео** одной правкой. Остался MarkdownV2 (НЕ HTML-флип) — у общих send-хелперов 4+ вызывающих (playlist/player) со своими подписями, флип parse_mode сломал бы их; полный HTML — отдельный follow-up. Разметка проверена на живом сервере (`sendMessage MarkdownV2` → `ok:true`). +1 unit-тест (бейдж/без-бейджа/санитайз backtick).

### Fixed
- **Rich messages (Bot API 10.1) — вскрыта реальная схема `sendRichMessage`, probe заработал** (v0.51.0-beta.11) — `/richtest` слал `rich_message={"blocks":[…]}` и сервер всегда отвечал `rich message must be non-empty`. Эмпирически (curl к живому local bot-api 10.1 в проде) + по исходнику `telegram-bot-api/Client.cpp::get_input_rich_message` выяснено: для **отправки** `InputRichMessage` — это `{"markdown": "<src>"}` ИЛИ `{"html": "<src>"}` (+ опц. `is_rtl`, `skip_entity_detection`); tdlib сам рендерит rich-блоки из markdown/html-источника. Сложные классы `RichBlock`/`RichText`/`RichBlockTable`/… — это формат **получения** (`JsonRichMessage`), не ввода. Фикс: `rich/mod.rs` `demo_payload()` → markdown-showcase (заголовок/таблица/цитата/код/список/ссылка); `send_rich_message(chat_id, {"markdown"|"html"})`. Проверено на проде: `{"ok":true}`, реальные rich-сообщения доставлены. 2 unit-теста обновлены. (Поддержанные rich-фишки: `#`-заголовки, таблицы, `>`-цитаты, списки, code-блоки, **bold**/_italic_/~~strike~~/`code`, ссылки.)

### Changed
- **Explore «Лента» — премиальный рендер: rich HTML-карточки + кнопка-на-трек** (v0.51.0-beta.10) — переписан вид таймлайна (было: плоский список + рассинхрон с номерными кнопками). Теперь каждый трек — **подробная HTML-карточка**: тип-эмодзи + `<b>артист</b> — title`, второй строкой моно-«бейдж» `<code>FORMAT · качество · размер · длительность</code>` + эмодзи-платформа (▶️ YouTube / ☁️ SoundCloud / 📸 Instagram / 🎵 TikTok / 🟢 Spotify / 🔗 …) + время. Каждой карточке соответствует **номер-эмодзи кнопка** (1️⃣…🔟) = мгновенный resend; крупный номер в теле и на кнопке → глаз не скачет. Переключено на `ParseMode::Html` (чище MarkdownV2, без эскейп-каши; inline-кнопки = plain, поэтому вся «красота» в теле). `TimelineEntry` расширен полями `format/size_bytes/duration_secs/video_quality/audio_bitrate` (всё уже было в `download_history` — без лишних запросов и миграций). Новые чистые хелперы `human_size`/`fmt_duration`/`platform_badge`/`number_emoji`/`tech_badge` + тесты (HTML-карточка с деталями, форматтеры). Mini-app позже отдельно; текстовая часть теперь «идеальная».

### Fixed
- **YouTube-скачивание упало после пересборки образа: deno 2.0.6 «unsupported» для yt-dlp 2026.06+** (v0.51.0-beta.9) — после редеплоя (свежий образ подтянул `yt-dlp 2026.06.12`) ВСЕ скачивания ютуба отдавали `No video formats found`. Диагностика на проде (`yt-dlp -v`): `JS runtimes: deno-2.0.6 (unsupported)` → `JS Challenge Providers: deno (unavailable)` — n-challenge (nsig) не решался ни с какими cookies/прокси (PO-токены через bgutil HTTP при этом работали). Alpine `edge/testing` отдаёт только deno **2.0.6** (ноя-2024), новее в apk нет, а yt-dlp 2026.06+ его уже не поддерживает. Фикс: переключить JS challenge runtime `--js-runtimes deno` → **`node`** (node 22 уже в образе и поддерживается). `deno,node` НЕ помогает — yt-dlp берёт deno первым и падает без fallback. Заменено во всех прод-местах `doracore`: `push_js_runtimes_tail` (хелпер, 6 call-site'ов скачивания), `metadata.rs` (×3: основной/artist/uploader пробы), `cookies/probes.rs` (×3 cookie-валидации). 2 unit-теста обновлены (`js_runtimes_tail_*`). **Проверено на Railway перед коммитом** (CLAUDE.md): `--js-runtimes node` → реальное скачивание `jNQXAC9IVRw` → mp3 PASS. `doratui` (Mac-приложение) не трогали — у него свой deno вне Alpine. Порядок остальных yt-dlp-аргументов не менялся.

### Added
- **Instagram Stories — выбор aspect ratio (фаза A)** (v0.51.0-beta.8) — Stories больше не зашит в 9:16: новый ряд на карточке выбирает целевой AR — **9:16 / 1:1 / 4:5 / 16:9 / original**. Режим Crop = центр-кроп (zoom-to-fill, режет края — «вижу только центр»), Blur = размытый фон. Обобщён ffmpeg-фильтр (вынесен `reframe_filter(reframe, w, h)`), `send_video` отдаёт width/height выбранного AR. **Оптимизация:** `original` + без reframe → `-c copy` (stream-copy, без перекодирования, почти мгновенно; сегменты режутся по существующим keyframe). Токен callback расширен `<mode><seg><quality><delivery><ar>` (обратно совместим: старые токены без AR-символа → 9:16). 5 новых unit-тестов (dims/token round-trip, AR parse/encode, reframe_filter под целевые dims, original stream-copy без фильтра). Фаза B (setup-wizard с сеткой-превью всех AR) — следующим заходом. Спека: `docs/superpowers/specs/2026-06-13-stories-aspect-ratio-wizard-design.md`.

### Security
- **openssl 0.10.78 → 0.10.81** (v0.51.0-beta.7) — закрывает 3 Dependabot-алерта на транзитивном `openssl`: **GHSA-xp3w-r5p5-63rr** (high) + **GHSA-xv59-967r-8726** (medium) + **GHSA-phqj-4mhp-q6mq** (medium). Только `Cargo.lock` (+`openssl-sys` 0.9.114→0.9.117); прямой зависимости нет, поведение не меняется, сборка зелёная.

### Added
- **Instagram Stories — выбор доставки: 📹 Видео / 📄 Файл** (v0.51.0-beta.6) — в конфиг-карточку Stories добавлен 4-й тумблер: слать ли каждый минутный сегмент как **проигрываемое видео** (`sendVideo`, как было — дефолт, поведение не меняется) или как **файл/документ** (`sendDocument`) — raw `.mp4` без пере-сжатия Telegram, чтобы перезаливать в Instagram в оригинальном качестве. Токен callback расширен `<mode><seg><quality>` → `<mode><seg><quality><delivery>` (`v`/`f`, напр. `b60sf`); `parse` переписан на позиционное сканирование и **обратно совместим** — старые токены без delivery-символа декодируются в `Video`. Цикл отправки ветвится по `settings.delivery`. i18n ×4 (`stories-delivery-video`/`stories-delivery-file`). +3 unit-теста (round-trip с delivery, legacy-токены → Video, toggle parse/encode). Без изменений ffmpeg-пасса (сегменты те же) → smoke-гейт не затрагивается.
- **Instagram Stories — конфиг-карточка: reframe / длина сегмента / качество** (v0.51.0-beta.5) — кнопка «📱 Instagram Stories» теперь открывает stateless-карточку выбора вместо мгновенного запуска (дефолты = старое поведение, так что это +1 тап). Три параметра: **reframe** — 🌫 Размытый фон (как было: клип целиком + boxblur-фон) ИЛИ 🔍 Обрезать (zoom-to-fill 9:16, режет края — «родной» вид сторис); **длина сегмента** — 15/30/60 c; **качество** — Стандарт (CRF 20 / preset medium / AAC 192k) ИЛИ Максимум (CRF 18 / preset slow / AAC 256k). Настройки кодируются прямо в callback (`downloads:stories:cfg|go:{id}:{flags}`, токен `<mode><seg><quality>` напр. `c30m`) — без БД и сессий; карточка живёт через `edit_md_kb`. Кап источника поднят 10→20 мин (`MAX_TOTAL_SECS` 600→1200, всё ещё trim-from-start с предупреждением; не убран полностью — безлимитный энкод на общем боксе = риск runaway, ср. инцидент 2026-03-09; 20 мин укладываются в 30-мин ffmpeg-таймаут даже на preset slow). Crop — одноцепочечный фильтр (`scale=…:force_original_aspect_ratio=increase,crop=1080:1920`). 13 unit-тестов (parse/encode round-trip + tolerance, crop vs blur фильтр, std/max args, seg propagation, segment muxer, cap). i18n ×4 (7 ключей `stories-config-*`/`stories-mode-*`/`stories-quality-*`/`stories-create`/`stories-cancel`). ffmpeg smoke-тест обоих новых фильтров на Railway перед коммитом. Самодостаточный модуль `telegram/downloads/stories.rs` — не трогает `process_video_clip`.

### Added
- **Rich text (Bot API 10.1) — probe-харнесс + апгрейд bot-api сервера** (v0.51.0-beta.4) — фундамент под rich-сообщения (`sendRichMessage`, таблицы/заголовки/сворачиваемые секции/математика и т.д.). teloxide (пин Bot API 8.x) их не знает → новый модуль `telegram/rich` шлёт JSON **сырым HTTP** (как guest-bots `answerGuestQuery`): `call_method(method, body)` + `send_rich_message(chat_id, value)` возвращают **полный ответ сервера**. Скрытая admin-команда `/richtest` отправляет кандидат-payload и отвечает админу сырым JSON-ответом — потому что точная схема `InputRichMessage` (фича от 2026-06-11) ещё не попала ни в machine-readable миррор, ни в teloxide, ни в извлекаемый вид офиц. доков; схема выясняется **эмпирически против живого 10.1-сервера**, затем пишется типизированный билдер. **Инфра:** Dockerfile запинен с плавающего `aiogram/telegram-bot-api:latest` на конкретный **digest 10.1** (`sha256:102a5878…`, 2026-06-13) — воспроизводимость + осознанный апгрейд сервера 10.0→10.1 (прод-сервер был 10.0, где `sendRichMessage` отсутствует). ⚠️ Апгрейд bot-api = рискованное изменение контейнера (ср. инцидент 2026-03-09): после деплоя обязательна проверка `telegram-bot-api --version`=10.1 + smoke-тест скачиваний. 2 unit-теста (demo_payload, api_base). Admin-only → ноль влияния на обычных юзеров.

### Added
- **Explore-хаб + вкладка «Лента» (Recent timeline)** (v0.51.0-beta.3) — новый inline-хаб `/explore` (+ кнопка главного меню «📜 Лента») показывает скачанное юзером в виде таймлайна, сгруппированного по датам (Сегодня / Вчера / Эта неделя / Этот месяц / Ранее) с мгновенным resend из кешированного Telegram `file_id` (без перекачки; диспатч по типу — audio/video/video_note/gif/document). Архитектура: единый backend `TimelineService` (`doracore::explore::timeline`) — чистые типы + хелперы (`bucket_for`, `group_into_buckets`, `paginate`, `media_kind_from_format`) с unit-тестами + async `build_timeline_page` поверх существующего `get_download_history_filtered` (пагинация в памяти, 10/стр). Тот же `TimelinePage` рендерится inline сейчас и отдаётся как JSON `GET /api/timeline?user_id=&page=` для будущего Telegram Mini App (stub-роут; auth через initData — в спеке Mini App). Inline-UI: таб-бар (Recent живой; **🔥 Тренды / ⭐ Подписки** — заглушки «🔜 Скоро» под под-проекты C/B), номерные resend-кнопки, пагинация `‹ ›`. Callbacks `exp:tab:* / exp:page:recent:N / exp:rs:ID`. Без миграций (читаем `download_history`). i18n ×4 (14 ключей `explore_*` + `bot_commands.explore`). **Известное ограничение:** при отсутствии/протухании `file_id` resend отправляет URL обычным сообщением (без тихого ре-download — нужно пробросить `download_queue` в хендлер; follow-up). Часть 1 из 3-частной декомпозиции Explore (A=хаб+Лента, B=Playlist Sync, C=Discovery-вкладки); спек и план — в `docs/superpowers/`.

### Fixed
- **Age-restricted (18+) видео выдавало «Temporary issue, retry later» + спамило админ-алерт** (v0.51.0-beta.2) — yt-dlp на 18+ ролике (`Sign in to confirm your age`) возвращает stderr, содержащий и `Use --cookies for the authentication`. Классификатор `analyze_ytdlp_error` ловил эту подстроку **раньше** и помечал ошибку как `InvalidCookies` → юзеру шло неверное «❌ Temporary issue with YouTube. Try a different video or retry later.» (хотя для age-gate ретраи бессмысленны — это навсегда без 18+-аккаунта), а `should_notify_admin=true` слал ложный HighErrorRate-алерт «cookies invalid» на каждую попытку. Эмпирически проверено в проде: age-gate **не пробивается** ни одним `player_client` (android_vr/tv/tv_embedded/mediaconnect/web/web_creator/mweb — все дают «Sign in to confirm your age»), даже с куками — текущий cookie-файл оказался неполным third-party экспортом без first-party логин-кук (`SID`, `SAPISID`, `__Secure-1PSID/1PAPISID`, `LOGIN_INFO`). Фикс: новый `YtDlpErrorType::AgeRestricted`, детектится **до** cookie-ветки по `confirm your age`/`age-restricted`/`inappropriate for some users`; точное user-сообщение «🔞 This video is age-restricted (18+)…»; `should_notify_admin=false` (per-video стена YouTube, не сбой сервиса); добавлен в `should_try_tier2` allow-list, чтобы при наличии **валидных** 18+-кук cookie-попытка (Tier 2) всё ещё запускалась; в `try_tier2` age-fail больше не дёргает futile cookie-refresh. Все exhaustive-`match` (metadata.rs, source/ytdlp.rs, pipeline.rs admin-карточка → «🔞 AGE-RESTRICTED (18+)») обновлены. 2 новых unit-теста. yt-dlp args не менялись (download/ smoke-test PASS).
- **В группах юзер регистрировался как «новый» (identity по chat.id вместо from.id)** (v0.51.0-alpha.37) — бот ключевал личность пользователя по `msg.chat.id`, который равен реальному Telegram user-id **только в личке** (в DM `chat.id == from.id`). В группе/канале `chat.id` — это отрицательный id чата, поэтому `schema.rs` создавал фиктивного «нового юзера» = группу (admin-уведомление с `-100…` id) и привязывал к нему историю/настройки, ломая совпадение с DM-личностью. Inline (`inline_query.rs`) уже корректно ключевал по `query.from.id`, поэтому его не трогали. Фикс (scope B — guard): stateful-обработка входящих сообщений (`command_handler`, `voice_message_handler`, `message_handler`) теперь гейтится новым фильтром `is_private_chat(msg)` → не-private чаты больше не создают юзеров и не запускают скачивание-по-сообщению. Группы/каналы пользуются ботом через inline-режим (он identity-чистый). Существующие DM-юзеры не затронуты (для них `chat.id` и так == `from.id`).
- **Inline mode не получал запросы в проде** (v0.51.0-alpha.36) — `setWebhook` не передавал явный `allowed_updates`, поэтому Telegram держал предыдущий список (зарегистрированный до того как BotFather inline-mode был включён), и `inline_query`/`chosen_inline_result` updates **никогда не доходили** до бота. Симптом: юзер набирает `@doradura_bot ...` → ни popup, ни логов на стороне бота. Фикс: (1) `webhook.rs::set_webhook` теперь всегда явно слал полный allow-list (message, edited_message, channel_post, edited_channel_post, callback_query, **inline_query**, **chosen_inline_result**, my_chat_member, chat_member, chat_join_request); (2) новая `ensure_webhook_config(bot)` — на каждом старте webhook-mode вызывается `getWebhookInfo`, и если `inline_query`/`chosen_inline_result` отсутствуют в live-конфиге → бот **сам** переустанавливает webhook. Self-heal предотвращает повторение если кто-то в будущем вручную вызовет `setWebhook` без флагов. Также `chosen_inline_result` теперь приходит в bot (handler пока no-op — добавится в beta.X для inline-аналитики).

### Added
- **Chapter timestamps в подписи отправленного видео/аудио** (v0.51.0-alpha.36) — когда у видео есть chapters (yt-dlp `chapters[]`) или таймстемпы в описании, они теперь рендерятся прямо в caption сразу под заголовком (`*Artist* — _Title_` + пустая строка + `MM:SS — Label` построчно). Источник — `PREVIEW_CACHE.timestamps`, который и так заполняется в preview-фазе через `extract_all_timestamps()` (chapter_parser + description_parser fallback) — никаких лишних yt-dlp вызовов. Cap: 10 entries (через `select_best_timestamps` для равномерного распределения по длине), label ≤50 chars (с `…`), общий блок ≤380 raw chars → итоговая caption гарантированно ≤1024 (Telegram-лимит) с учётом MarkdownV2-эскейпов + copyright signature. Cache miss → graceful fallback на старую caption без таймстемпов. Новая публичная функция `doracore::core::utils::format_media_caption_with_chapters(title, artist, &[VideoTimestamp])` + 6 unit-тестов (renders block, empty fallback, skips empty labels, escapes special chars, truncates long label, caps total count). Работает и для MP3 (полезно для подкастов/миксов), не только MP4. URL-пример: `https://www.youtube.com/watch?v=JFG9GJxOivg`.

- **Inline-режим personal-first для URL + bulletproof history saves** (v0.51.0-alpha.35) — теперь при `@doradura_bot https://yt.be/x` СНАЧАЛА показываются ТВОИ собственные cached file_ids этого URL из `download_history`, и только потом supplement из глобального `popular_files` (с de-dup по формату — твой mp3 вытесняет чужой mp3, но чужой mp4 поднимается если ты сам не качал). UX-эффект: «я скачал это вчера — вижу свою копию в inline». Новый targeted accessor `get_user_history_for_url(user_id, url)` (SQLite + Postgres, `file_id IS NOT NULL`, `ORDER BY downloaded_at DESC`) + 4 unit-теста. Pure stitch-helper `stitch_url_results` + 4 unit-теста на personal-first ordering и format-dedup.

### Changed
- **Speed-modify и burn-subs пути теперь всегда записывают в `download_history`** (v0.51.0-alpha.35) — раньше `speed.rs:142/231` и `voice_lyrics.rs:265` имели `if let Some(fid) = new_file_id` guard вокруг `save_download_history` → если Telegram вернул ответ без распознаваемого `file_id` (редкий случай malformed response), запись истории молча терялась, юзер скачал — а в `/history` и inline этого файла нет. Теперь save безусловный, при `file_id=None` пишем NULL и `log::warn!` с заголовком — inline-фильтр всё равно отсеет NULL, но запись видна в админских отчётах и для будущего MTProto refresh.

### Added
- **Inline-режим top-UX: личный поиск + recents + multi-format + funnel-кнопка** (v0.51.0-alpha.34) — переработка `@doradura_bot <…>` в трёхрежимный диспетчер. `@bot ` (пусто) → твои последние 15 скачиваний (CachedAudio/Video/Gif). `@bot Дора Дорадура` → поиск по `download_history.title/author` (AND если «Author - Title», OR если просто слова) с Vlipsy GIF как fallback. `@bot https://yt.be/x` → ВСЕ закешированные форматы из `popular_files` за один запрос (mp3+mp4+m4r+video_note+gif+cut, не только mp3+mp4). Везде сверху постоянная кнопка **🔽 Открыть Doradura** через новый `InlineQueryResultsButton::StartParameter("from_inline")` API. Богатые caption/description с duration · bitrate/quality · size. Article-fallback с YouTube-thumbnail. **Bugfix:** URL-lookup теперь канонизирует ссылку (`canonicalize_url`) перед обращением к `popular_files` — раньше любой `?si=…` вариант ютуб-ссылки мимо кеша. Новый accessor `lookup_popular_file_all_formats` (один запрос вместо N round-trip). 18 новых unit-тестов.

### Changed
- **i18n для Silent downloads и Instagram Stories** (v0.51.0-alpha.33) — все строки обеих фич (тоггл/алерты/MOTD-сводка/статусы/ошибки/подписи) переведены и вынесены в 4 локали (en/ru/fr/de), 20 новых fluent-ключей `silent-*` и `stories-*` вместо захардкоженного русского. MOTD-заголовки используют `t_args` с counts. Примечание: fluent isolation-маркеры (U+2068/U+2069) вокруг `{ $arg }` — невидимы в Telegram, безвредны.

### Added
- **Silent downloads + MOTD-сводка** (v0.51.0-alpha.32) — персональный тоггл «🔇 Тихие загрузки» (в Settings и кнопкой прямо на превью-карточке). Когда включён: загрузка ставится с низким приоритетом (обычные всегда обгоняют — «как будет время»), идёт **без сообщений** (ни позиции в очереди, ни прогресс-бара, ни подписи/signoff/share), доставляется с `disable_notification` (без пинга), а приёмка подтверждается реакцией 👌 на исходное сообщение. Готовые (и упавшие) тихие загрузки копятся в таблице `silent_digest` и при **следующем обращении** к боту показываются одной MOTD-сводкой «📬 Пока тебя не было — готово N: …», после чего помечаются как показанные (атомарный `UPDATE … RETURNING`, идемпотентно — две быстрые активности не задвоят сводку). Миграция `V49` (`users.silent_downloads` + `silent_digest`), флаг читается воркером в момент обработки — без изменения схемы `task_queue`.
- **Instagram Stories — нарезка клипа в вертикальный 9:16** (v0.51.0-alpha.31) — на любом скачанном MP4 (кнопка под видео, в меню resend и в категориях) появилась «📱 Instagram Stories». Один ffmpeg-pass: клип вписывается по центру кадра 1080×1920, фон — размытая (boxblur) + слегка затемнённая копия того же кадра (фирменный Reels-вид, без чёрных полос), результат режется segment-муксером на сегменты по 60 c (лимит истории) с принудительными keyframes на границах. Каждый сегмент уходит отдельным портретным видео. Источники длиннее 10 мин обрезаются с начала. Самодостаточный модуль `telegram/downloads/stories.rs` — переиспользует общие download/ffmpeg/send хелперы, не трогает `process_video_clip`.
- **Inline mode для URL — `@doradura_bot https://yt.be/x` в любом DM/группе/канале** (v0.51.0-alpha.30) — комплимент к alpha.29 Guest Bots: работает в **private DMs** где guest_message не применим. Layout inline-результатов: (1) `InlineQueryResultCachedAudio` из popular_files (Path C, мгновенно), (2) `InlineQueryResultCachedVideo` из popular_files, (3) всегда `InlineQueryResultArticle` с deep-link `?start=dl_<urlid>_p` как fallback для нового URL. Vlipsy reaction search (free text) не тронут — старое поведение работает как раньше. Минимальная реализация: ~80 LOC + 1 регистрация в schema.rs.
- **Guest Bots (Bot API 10.0) — виральная воронка через @-mention в чужих чатах** (v0.51.0-alpha.29) — теперь в любой группе/канале где бота нет можно написать `@doradura_bot mp3` в реплай на YouTube-ссылку и получить ответ. **Lookup chain:** (1) global `popular_files` cache (V48) → если кто-либо когда-либо качал этот URL, отдаём `InlineQueryResultCachedAudio` за ~1с; (2) личная история caller'а → write-through в global cache; (3) `InlineQueryResultArticle` с deep-link `?start=dl_<urlid>_<a|v|p>` → юзер открывает DM, стандартный download pipeline. Webhook-mode только (polling fork — следующий sprint). teloxide master ещё не expose-ит `guest_message`, поэтому intercept через raw JSON в `dedup_middleware` + raw HTTP POST к `answerGuestQuery`. Anti-spam 5 req/min per (chat, user). Каждый успешный download автоматически апсёртит `popular_files` → каждый юзер вносит в общий cache.
- **Period & smart search в истории** (v0.51.0-alpha.28) — над списком `/downloads` (и кнопки 📚 в главном меню) появилась строка `[Today][7d][30d][All]`. Поиск понимает формат `"Дора - Дорадура"` — split на artist + title и AND-матч (раньше только OR по обеим колонкам). Главное меню → 📚 теперь ведёт в `/downloads` (а не в простой list-only view) — единый flow с фильтрами и per-item кнопками. Старые callbacks без period поля остаются рабочими (backwards compat).
- **Реальный progress bar для /circle, ringtone, GIF, cut** (v0.51.0-alpha.27) — раньше показывал «🎬 Encoding circle… 6s elapsed» без процентов. Теперь `▰▰▰▰▰▱▱▱▱▱ 50% · 12s/24s`. Парсится из ffmpeg `-progress pipe:1`. Retry paths остаются на elapsed-only (fallback).
- **Pin teloxide to upstream master** (v0.51.0-alpha.26) — снять с устаревшего 0.17.0; стабы для Bot API 8.x методов; `teloxide_tests` отключён до выхода 0.18 (integration test удалён, 605 unit tests остаются).
- **Long-video gate** (v0.51.0-alpha.17) — для видео ≥2h показываем интерактивную панель (audio/continue/range/cancel) вместо тихого роутинга в multi-GB download.
- **Universal upload-size validator** (v0.51.0-alpha.18) — единый источник правды по Telegram-лимитам (sendVideo / sendDocument / sendPhoto / sendVideoNote / sendVoice).
- **Прогресс-bar для всех ffmpeg операций** (v0.50.0/0.50.1/v0.51.0-alpha.13/19/20) — circle, cut, ringtone, GIF, audio effects, voice effects, speed change. Раньше зависал, теперь видно «🎬 Encoding circle… 12s elapsed».
- **Кнопка «❌ Cancel» во время скачивания** (v0.48.0) — особенно нужна на длинных Master 4K энкодах (50-80 мин).
- **Cut-interval preview с подтверждением** (v0.47.0) — перед запуском ffmpeg показываем «📋 Result: 65 sec (2 segments)» + кнопки Cut/Cancel.
- **Per-user video quality preset** (v0.46.0) — Balanced / Transparent / Master / Lossless tiers в Settings → Video quality.
- **2K / 4K / 8K downloads** (v0.41.0) — добавлены resolutions выше 1080p (требуют AV1/VP9 от YouTube).
- **Cut-выбор языка дорожки** (v0.33.0) — для видео с несколькими аудио-дорожками (оригинал + дубляж) показывается «🔊 Audio track» в превью.
- **Loop to audio** (v0.38.0) — кнопка «🔁 Loop to audio» на любом MP4: загружаешь MP3 → бот возвращает MP4 где видео-кусок зацикливается под полную длину песни.
- **Info submenu в превью карточке** (v0.51.0-alpha.1/.2/.3) — кнопка 📋 Инфо открывает thumbnail / geo-availability / metadata cards без скачивания.
- **Geo-card с allowlist** (v0.51.0-alpha.5) — для видео доступных только в N странах: «Доступно ТОЛЬКО в 🇺🇸 US, 🇨🇦 GB» (раньше показывало просто «geo-blocked»).
- **Lyrics smart-cascade** (v0.51.0-alpha.7) — поиск лирики через title-parser (forward/reverse splits, feat-stripping) для re-upload каналов где artist в названии.
- **Rich upload info card** (v0.51.0-alpha.10) — для загруженных файлов показываем resolution × duration × size + format/filename/date.
- **Per-platform ringtone selector** (v0.31.0) — iPhone `.m4r` / Android `.mp3` платформа-aware.
- **Health monitor service** (v0.31.0) — отдельный s6-сервис проверяет /health, переключает avatar online↔offline.
- **Caption ON/OFF toggle для видео** (v0.40.0) — бот может отправлять видео без подписи (для пересылки без брендинга).
- **Disk-cleanup background task** (v0.40.1) — каждые 6h чистит /data/downloads, retention 1 день (раньше копилось до 6+GB).
- **Подсказка «Choose how info» при длинных лекциях/подкастах** — uses long-video gate (alpha.17).
- **`mimalloc` global allocator** (v0.50.6) — 10-25% быстрее на alloc-heavy путях (ffmpeg log parsing, regex).
- **`/test_circle` admin command** (v0.43.2/.3) — для эмпирического тюнинга video-note encoder против Telegram transcoder.
- **`/update_health_check` admin command** (v0.43.0) — ручной триггер health probe + avatar refresh.
- **Cookies age-gated probe** (v0.39.0) — отдельный 5-min probe возрастно-ограниченного видео; admin notification только на edge-transitions.
- **Aggressive x264 tuning** (v0.50.2) — experimental flag в Settings, 1.75× faster encode at -1 VMAF (visually undetectable).

### Changed
- **/circle на 60s+ → multi-circle split** (v0.51.0-alpha.24) — раньше тихо обрезалось до первой минуты, теперь приходит до 6 кружков подряд.
- **4K через VP9 1:1 без перекодирования** (v0.51.0-alpha.23) — раньше yt-dlp выбирал AV1 → libx264 шакалил, теперь VP9 → `-c copy`, pristine quality.
- **Multi-circle quality fix** (v0.51.0-alpha.24) — split-pass теперь использует тот же preset что main pass (раньше ultrafast убивал качество).
- **Lyrics picker UX** (v0.51.0-alpha.16) — auto-apply короткие тексты (≤900 chars), smart-segment unstructured lyrics, превью первой строки на кнопках.
- **Validator Phase 1 sweep** (v0.51.0-alpha.20) — 6 хардкодов size-cap → typed UploadLimits (включая stale 50 MB hardcode'ы ломавшие local Bot API).
- **Master preset = `slow / CRF 14`** (v0.48.0/.1/.2) — финальный tier после серии итераций; ~99 VMAF, 5-10 мин на 1440p.
- **H.264 level 4.2 → 5.1** (v0.46.1) — фиксит 4K@60 frame drop и CRF-12 bitrate clamping.
- **Codec-aware skip H.264/VP9** (v0.49.0) — при VP9 source делаем `-c copy` remux вместо libx264 recode → 1440p за 30s вместо 5-10 мин.
- **Disk hygiene** (v0.49.1) — post-send cleanup 10min → 2min, retention 7d → 1d (фикс 6 GB orphaned mp4 на /data).
- **DOWNLOAD_LIMIT_DURATION** (v0.50.7+) — N+1 query collapse, queue wait-time metric, cargo-audit CI.
- **Hot-path perf cleanups** (v0.50.5) — N+1 collapse через `VideoDownloadSettings` bundle, request `Arc::clone`, sync I/O в spawn_blocking, codec-aware encode params.
- **Build profile tuning** (v0.50.3) — dev opt-level 0→1, deps opt-level 3, release LTO + strip.
- **Production lint baseline** (v0.50.4) — `unwrap_used` / `panic` / `unsafe_code` как workspace warn-level lints.
- **Edition 2024 migration** (v0.44.0) — workspace bumped to Rust 2024.
- **Refactor god-functions** (v0.38.5/.6/.7/.8/.9/.10/.11/.14/.15/.16, v0.38.20/.21/.23, v0.51.0-alpha.12/.13) — handle_message/handle_menu_callback/run_loop/handle_cuts/handle_videos/handle_settings/process_video_clip/circle.rs decomposed.
- **Cookies refactor 2243 LOC → 7 modules** (v0.39.3) — types/file_ops/probes/watchdog/manager/instagram/mod facade. Zero behavior change.
- **`bon::Builder` для DownloadTask** (v0.36.16) — заменил 3 positional конструктора (7-9 args каждый) на typed builder.
- **`CallbackKind` enum** (v0.38.3) — strum::EnumString парсит callback prefix; typo = compile error вместо silent miss.
- **Async Mutex → std Mutex для queue timestamp** (v0.38.2) — 16-byte critical section без `.await`, async overhead был лишним.
- **Doc coverage** (v0.41.0) — 26 новых rustdocs на storage::db::sessions/task_queue.
- **`build_common_args` deduplication** (v0.36.13) — eliminate copy-paste prefix между audio/video tier closures.
- **Inline HTML → `include_str!`** (v0.36.12) — admin_login / privacy / share страницы вынесены в .html файлы.
- **`BotExt` trait** (v0.36.11) — 4 helper-методов для MarkdownV2 send/edit boilerplate.
- **Prune phantom deps** (v0.36.17) — dropped tonic/prost/tower-http/shell-escape/tokio-retry; thiserror 1→2; strum 0.26→0.27.
- **Drop 17 unused workspace deps** (v0.51.0-alpha.10) — cargo-machete clean.
- **DX micro-bumps** (v0.51.0-alpha.10) — `pretty_assertions` wired in test mods, `cargo-sweep` cron wrapper.
- **Drop multi-circle split** (v0.44.1) — позже re-enabled в alpha.24.
- **Drop H.264 recode for 1440p+** (v0.45.0) — затем reverted в v0.45.1 (AV1 не играет inline в Telegram).
- **Restore H.264 recode** (v0.45.1) — slow + tune film + CRF 14.
- **Drop preset slow → medium** (v0.45.3) — slow OOM'd на Railway 4K.
- **Adaptive x264 preset для video-note encoding** (v0.42.4) — high-res = `slow`, ≤1080p = `fast`.
- **Stop fighting Telegram's server transcoder** (v0.43.2) — упростили video-note encode (Telegram всё равно re-encode'ит).
- **Roll back video-note preset → veryslow** (v0.43.3) — Telegram transcoder сжимает фастом, наш intermediate качество matters.
- **Restore `-profile:v high -level 4.0 -g 48 -keyint_min 24`** (v0.43.4) — empirically test_small.mp4 показал что эти флаги matters.
- **Lower video-note encoder memory** (v0.43.6) — Railway OOM dodge через filter graph rewrite + thread limits.

### Fixed
- **/downloads buttons показывают тип файла** (v0.51.0-alpha.25) — раньше все строки `📤 Title`, теперь `🎵 Title · MP3` / `⭕️ Title · Circle` / `🔔 Title · Ringtone`. Cuts resend больше не silent fail.
- **4K и другие high-qualities появились в превью** (v0.51.0-alpha.22) — раньше max-формат хидился из-за `max_formats=4` cap + random HashMap order для "best".
- **/circle truncation message** (v0.51.0-alpha.21) — раньше говорил «for ringtones (30 sec) (40s). First 60 seconds» (3 разных числа из 3 разных мест), теперь kind-aware и корректное число.
- **Lyrics с кэшированных MP3** (v0.51.0-alpha.11/.15) — `with_lyrics` flag тёрся при queue persistence (alpha.15 V47 migration), title/artist re-hydrate в cache-hit branches (alpha.11).
- **Lyrics fallback при Genius miss** (v0.51.0-alpha.6) — для re-upload каналов retry title-only; explicit «📝 Не удалось найти» вместо silent return.
- **Cancel button neutral message** (v0.51.0-alpha.14) — раньше «Download error: Download error: Cancelled by user» + sticker + admin alert. Теперь нейтральное «❌ Download cancelled.»
- **Portrait video thumbnails** (v0.51.0-alpha.14) — для вертикальных видео thumbnail был landscape (yt-dlp всегда даёт hqdefault.jpg 1280×720). Фикс: skip explicit thumb для portrait, Telegram сам генерирует.
- **Geo-blocked видео не висит queue 4 минуты** (v0.38.12) — yt-dlp distinguish geo-block от video unavailable, fast-fallback на следующий proxy.
- **Vertical видео не растягивается в landscape** (v0.37.0) — ffprobe теперь читает `rotation` из side_data_list + tags, swap width↔height для 90°/270°.
- **Circle 4K silently lost audio after OOM** (v0.42.5) — retry path использовал video-only filter; смягчили preset + retry с full audio+video.
- **Circle blurry на 4K source** (v0.42.3) — добавили `flags=lanczos` + preset medium → fast.
- **send_video document fallback использовал 50 MB cap** (v0.42.2) — на local Bot API лимит 5 GB; перешли на dynamic ceiling.
- **High-res mp4 actually re-encodes to H.264** (v0.42.1) — yt-dlp skip'ал re-encode при mp4→mp4, фикс через mkv intermediate.
- **High-res disk floor** (v0.41.2) — 8 GB hardcoded floor ломал downloads на small Railway volumes; теперь env-configurable (default 2 GB).
- **Preview metadata: switch player_client** (v0.41.1) — для exposed 1440p/2160p/4320p в preview keyboard.
- **Кружок сохранил quality на Railway OOM** (v0.43.5) — smart retry с medium preset вместо ultrafast.
- **Cookie validation report не показывает legacy red ❌** (v0.36.14) — modern Chrome cookies (`__Secure-3PSID`) теперь распознаются.
- **`busy_timeout` SQLite 5s → 30s** (v0.38.25) — un-jam queue во время long downloads.
- **Preview format filter unhide 720p/1080p** (v0.38.24) — на local Bot API использовал dynamic ceiling вместо 2 GB hardcode.
- **Multi-instance orphan ffmpeg/yt-dlp kill on startup** (v0.49.2) — фикс 9+ GB peak RAM от двух ffmpeg в параллель после restart.
- **WITH_COOKIES fallback missing cache write** (v0.33.1) — `--load-info-json` теперь работает даже когда первая proxy attempt failed.
- **Diagnostic logs для lyrics path** (v0.51.0-alpha.14) — INFO logs на decision points чтобы next reproduction показал умирающую ветку.
- **CI Lint suppressions** (v0.39.1, v0.38.22) — Rust 1.95 clippy `collapsible_match` в TUI/subscriptions.rs.
- **CI Env Schema drift** (v0.40.2) — `DOWNLOADS_RETENTION_DAYS` добавлен в .env.schema.
- **Test_format_duration off-by-one** (v0.38.13) — тест ожидал «61:01» вместо корректного «1:01:01».
- **Hide 4320p (8K) option** (v0.43.7) — empirically broken на Railway, downgrade silently → 2160p.
- **Don't flip to «Merging 0%»** (v0.45.2) — gate behind real ffmpeg progress.

### Removed
- **`teloxide_tests` dev-dep + integration test файл** (v0.51.0-alpha.26) — fork не tracks teloxide master; будет восстановлен когда выйдет 0.18.

### Security
- **Dependabot: close 8/12 advisories** (v0.39.2) — openssl 0.10.76→0.10.78 (4 HIGH), actix-http 3.12.0→3.12.1 (1 MEDIUM), rustls-webpki 0.103.10→0.103.13 (2 LOW), rand updates (1 LOW).
- **Bump `wiremock` 0.5 → 0.6** (v0.51.0-alpha.8) — drops vulnerable transitive `rand` 0.7.3.
- **`cargo-deny` supply-chain guard** (v0.51.0-alpha.13) — pre-commit + CI advisory/license/bans проверки.
- **varlock Phase A/B/C** (v0.38.4 + ranges) — env schema validation: 16 missing vars added, fatal boot-time validation, CI drift check.

## [0.33.1] - 2026-03-30

### Fixed
- **WITH_COOKIES fallback missing cache write** — `--load-info-json` optimization now works even when first proxy attempt fails (common on Railway)

### Changed
- Extract `pot_for_experimental()` helper — eliminates 8× duplicated POT logic across download tiers
- Extract `youtube_info_cache_path()` to `core::share` — single source of truth for cache path across crate boundary
- Remove redundant comments that paraphrase code

## [0.33.0] - 2026-03-23

### Added
- **Audio track language selection** for video downloads — YouTube videos with multiple audio tracks (original + dubbed) now show a `🔊 Audio track` button in the preview keyboard. Users can pick which language track to download (e.g., Japanese original vs English dub). Selection is stored per-URL and passed to yt-dlp via `[language=XX]` format filter with automatic fallback to best audio.

## [0.31.1] - 2026-03-20

### Fixed
- Download queue completely broken: V19 migration "duplicate column" error caused refinery to roll back entire batch, skipping V39 (task_queue columns). All `save_task_to_queue` and `claim_next_task` calls failed silently
- Pre-apply problematic ALTER TABLE statements from V19/V26 before refinery runs
- `ensure_tables()` now idempotently creates V39 columns on `task_queue` and `processed_updates` table

## [0.31.0] - 2026-03-19

### Added
- Multi-instance runtime with Postgres backend and Redis queue (PR #18)
- `SharedStorage` abstraction — SQLite for dev, Postgres+Redis for production
- `DATABASE_DRIVER` env var to switch between `sqlite` and `postgres`
- Tracing spans with per-task operation IDs for log correlation
- Health monitor crate — auto-recovers bot title, checks `/health`
- Archive ZIP download of user history
- `TempDirGuard` RAII wrapper — eliminates ~40 manual temp file cleanups
- Prometheus `/metrics` endpoint with all download/send/error counters
- Ringtone platform selector (iPhone `.m4r` / Android `.mp3`)

### Changed
- Axum upgraded to 0.8 (path params `{id}` syntax)
- Download module refactored to trait-based `DownloadSource` + `SourceRegistry`

### Fixed
- Axum 0.8 path param syntax (`:id` -> `{id}`) — fixed web server panic
- Tracing subscriber init made non-fatal to prevent crash loops
- Health monitor respects Telegram rate limits, no longer burns `setMyName`
- Archive tables ensured after migration rollback

## [0.30.1] - 2026-03-12

### Fixed
- Dockerfile builder removed from `railway.json`, using GHCR image source
- `set_global_default` + `LogTracer` used separately to avoid log conflict
- `LogTracer::init()` removed — conflicted with tracing-subscriber

## [0.30.0] - 2026-03-10

### Added
- Detailed API logging in health monitor with Retry-After visibility
- URL allowlist enforcement on both preview and download paths

### Fixed
- Health monitor no longer burns `setMyName` rate limit on deploy
- Dependencies updated (quinn-proto CVE, 113 packages)

### Changed
- ~5,400 lines of doracore/dorabot code duplication eliminated

[Unreleased]: https://github.com/Jacke/doradura/compare/v0.31.1...HEAD
[0.31.1]: https://github.com/Jacke/doradura/compare/v0.31.0...v0.31.1
[0.31.0]: https://github.com/Jacke/doradura/compare/v0.30.1...v0.31.0
[0.30.1]: https://github.com/Jacke/doradura/compare/v0.30.0...v0.30.1
[0.30.0]: https://github.com/Jacke/doradura/releases/tag/v0.30.0
