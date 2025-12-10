const tg = window.Telegram.WebApp;
tg.ready();
tg.expand();

const state = {
    userId: tg.initDataUnsafe?.user?.id ?? null,
    settings: null,
    stats: null,
    history: [],
    services: [],
    queue: [],
    preview: null,
    selectedFormat: 'mp3',
    selectedVideoQuality: '720p',
    selectedAudioBitrate: '320k',
    sendAsDocument: false,
    sendAudioAsDocument: false,
    loading: false,
    queuePollId: null,
    planUpdateInProgress: false,
};

const PLAN_CONFIG = {
    free: {
        label: 'Free',
        icon: '🌱',
        limits: 'до 25 загрузок в сутки',
        quotaLabel: 'Лимит: 25/день',
        features: [
            'Обычная очередь',
            'MP3 до 192 kbps',
            'Видео до 720p',
        ],
    },
    premium: {
        label: 'Premium',
        icon: '🚀',
        limits: 'до 150 загрузок в сутки',
        quotaLabel: 'Лимит: 150/день',
        features: [
            'Приоритетная очередь',
            'MP3 до 320 kbps',
            'Видео до 1080p',
            'Сервисы TikTok, Twitch, Reddit',
        ],
    },
    vip: {
        label: 'VIP',
        icon: '💎',
        limits: 'безлимит, отдельный сервер',
        quotaLabel: 'Без лимитов',
        features: [
            'Отдельный воркер',
            'Видео до 4K',
            'Аудио FLAC/320k',
            'Поддержка 24/7',
        ],
    },
};

const refs = {
    userDisplay: document.getElementById('userDisplay'),
    userHint: document.getElementById('userHint'),
    planBadgeLabel: document.getElementById('planBadgeLabel'),
    planLimits: document.getElementById('planLimits'),
    planBadge: document.getElementById('planBadge'),
    requestQuota: document.getElementById('requestQuota'),
    urlInput: document.getElementById('urlInput'),
    downloadBtn: document.getElementById('downloadBtn'),
    previewBtn: document.getElementById('previewBtn'),
    previewCard: document.getElementById('previewCard'),
    previewTitle: document.getElementById('previewTitle'),
    previewMeta: document.getElementById('previewMeta'),
    formatButtons: Array.from(document.querySelectorAll('#formatButtons .format-btn')),
    videoQualityButtons: Array.from(document.querySelectorAll('#videoQualityButtons .quality-btn')),
    audioQualityButtons: Array.from(document.querySelectorAll('#audioQualityButtons .quality-btn')),
    videoQualityBlock: document.getElementById('videoQualityBlock'),
    audioQualityBlock: document.getElementById('audioQualityBlock'),
    navButtons: Array.from(document.querySelectorAll('.nav-btn')),
    screens: Array.from(document.querySelectorAll('.screen')),
    profileUsername: document.getElementById('profileUsername'),
    profileUserId: document.getElementById('profileUserId'),
    profileJoined: document.getElementById('profileJoined'),
    profilePlan: document.getElementById('profilePlan'),
    planFeatures: document.getElementById('planFeatures'),
    statsTotal: document.getElementById('statsTotal'),
    statsSuccess: document.getElementById('statsSuccess'),
    statsFailed: document.getElementById('statsFailed'),
    statsSize: document.getElementById('statsSize'),
    successProgress: document.getElementById('successProgress'),
    historyList: document.getElementById('historyList'),
    historyEmpty: document.getElementById('historyEmpty'),
    queueList: document.getElementById('queueList'),
    queueEmpty: document.getElementById('queueEmpty'),
    queueUpdateIndicator: document.getElementById('queueUpdateIndicator'),
    toggleVideoDoc: document.getElementById('toggleVideoDoc'),
    toggleAudioDoc: document.getElementById('toggleAudioDoc'),
    serviceList: document.getElementById('serviceList'),
    planButtons: Array.from(document.querySelectorAll('[data-plan-option]')),
    cancelSubscriptionBtn: document.getElementById('cancelSubscriptionBtn'),
    planUpgradeButtons: Array.from(document.querySelectorAll('.plan-upgrade-btn')),
    adminNavBtn: document.getElementById('adminNavBtn'),
    adminTotalUsers: document.getElementById('adminTotalUsers'),
    adminTotalDownloads: document.getElementById('adminTotalDownloads'),
    adminActiveQueue: document.getElementById('adminActiveQueue'),
    adminTotalSize: document.getElementById('adminTotalSize'),
    adminPlanFree: document.getElementById('adminPlanFree'),
    adminPlanPremium: document.getElementById('adminPlanPremium'),
    adminPlanVip: document.getElementById('adminPlanVip'),
    adminQueueList: document.getElementById('adminQueueList'),
    adminQueueEmpty: document.getElementById('adminQueueEmpty'),
};

function init() {
    if (!state.userId) {
        notifyError('Не удалось получить user_id. Запусти Mini App из Telegram.');
        refs.downloadBtn.disabled = true;
        refs.previewBtn.disabled = true;
        return;
    }

    setupNavigation();
    setupFormatControls();
    setupQualityControls();
    setupSettingsControls();
    setupPlanSwitcher();
    setupUpgradeButtons();
    setupButtons();
    hydrateUserMeta();
    checkAdminAccess();
    setSelectedFormat(state.selectedFormat);
    setActiveQuality(refs.videoQualityButtons, state.selectedVideoQuality, 'quality');
    setActiveQuality(refs.audioQualityButtons, state.selectedAudioBitrate, 'bitrate');
    bootstrap();
    window.addEventListener('beforeunload', stopQueuePolling);
}

function hydrateUserMeta() {
    const user = tg.initDataUnsafe?.user;
    if (!user) {
        refs.userDisplay.textContent = 'Гость';
        refs.userHint.textContent = 'авторизация не выполнена';
        return;
    }

    const displayName = user.username ? `@${user.username}` : `${user.first_name ?? ''} ${user.last_name ?? ''}`.trim();
    refs.userDisplay.textContent = displayName || 'Пользователь';
    refs.userHint.textContent = `ID ${user.id}`;

    refs.profileUsername.textContent = displayName || '—';
    refs.profileUserId.textContent = `ID: ${user.id}`;
    refs.profileJoined.textContent = `Интерфейс: Telegram ${user.language_code?.toUpperCase() ?? ''}`;
}

function checkAdminAccess() {
    // Check if the user is an admin
    // ADMIN_IDS must be defined on the server side
    // Here we simply check for the presence of the admin tab in settings
    fetchSettings().then(settings => {
        if (settings?.is_admin) {
            refs.adminNavBtn.style.display = 'block';
        }
    }).catch(() => {
        // Do nothing if not admin
    });
}

function setupNavigation() {
    refs.navButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            const target = btn.dataset.screen;
            refs.navButtons.forEach((item) => item.classList.toggle('active', item === btn));
            refs.screens.forEach((screen) => {
                const isActive = screen.id === `screen-${target}`;
                screen.classList.toggle('active', isActive);
            });

            // Show the main button only on the main screen
            if (target === 'home') {
                tg.MainButton.show();
            } else {
                tg.MainButton.hide();
            }

            // Load admin data when the tab opens
            if (target === 'admin') {
                loadAdminData();
            }

            tg.HapticFeedback.impactOccurred('soft');
        });
    });
}

function setupFormatControls() {
    refs.formatButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            setSelectedFormat(btn.dataset.format);
            tg.HapticFeedback.impactOccurred('light');
        });
    });
}

function setupQualityControls() {
    refs.videoQualityButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            refs.videoQualityButtons.forEach((b) => b.classList.remove('active'));
            btn.classList.add('active');
            state.selectedVideoQuality = btn.dataset.quality;
            tg.HapticFeedback.impactOccurred('light');
        });
    });

    refs.audioQualityButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            refs.audioQualityButtons.forEach((b) => b.classList.remove('active'));
            btn.classList.add('active');
            state.selectedAudioBitrate = btn.dataset.bitrate;
            tg.HapticFeedback.impactOccurred('light');
        });
    });
}

function setupSettingsControls() {
    refs.toggleVideoDoc.addEventListener('change', () => {
        state.sendAsDocument = refs.toggleVideoDoc.checked;
        updateSettingsOnServer({ send_as_document: state.sendAsDocument });
    });

    refs.toggleAudioDoc.addEventListener('change', () => {
        state.sendAudioAsDocument = refs.toggleAudioDoc.checked;
        updateSettingsOnServer({ send_audio_as_document: state.sendAudioAsDocument });
    });
}

function setupPlanSwitcher() {
    refs.planButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            const plan = btn.dataset.planOption;
            if (!plan || plan === state.settings?.plan || state.planUpdateInProgress) {
                return;
            }
            handlePlanChange(plan);
        });
    });

    refs.cancelSubscriptionBtn.addEventListener('click', handleCancelSubscription);
}

function setupUpgradeButtons() {
    refs.planUpgradeButtons.forEach((btn) => {
        btn.addEventListener('click', () => {
            const plan = btn.dataset.plan;
            handleUpgradeRequest(plan);
        });
    });
}

function handleUpgradeRequest(plan) {
    const planName = PLAN_CONFIG[plan]?.label ?? plan;

    // Show confirmation before sending
    tg.showConfirm(
        `Подключить тариф ${planName}?\n\nБот отправит вам информацию о подключении подписки.`,
        (confirmed) => {
            if (confirmed) {
                // Send data to the bot via Web App API
                // The bot receives this as web_app_data
                tg.sendData(JSON.stringify({
                    action: 'upgrade_plan',
                    plan: plan
                }));

                tg.HapticFeedback?.notificationOccurred?.('success');
            }
        }
    );

    tg.HapticFeedback?.impactOccurred?.('medium');
}

function setupButtons() {
    refs.previewBtn.addEventListener('click', handlePreview);
    refs.downloadBtn.addEventListener('click', handleDownload);
    refs.urlInput.addEventListener('input', () => updateUrlFieldState());

    tg.MainButton.text = '📥 Скачать';
    tg.MainButton.onClick(() => refs.downloadBtn.click());
    tg.MainButton.show();
}

function updateUrlFieldState() {
    const value = refs.urlInput.value.trim();
    refs.urlInput.style.borderColor = value && !isValidUrl(value) ? '#ff6b6b' : 'var(--border-color-dark)';
}

function setSelectedFormat(format) {
    state.selectedFormat = format;
    refs.formatButtons.forEach((btn) => {
        btn.classList.toggle('active', btn.dataset.format === format);
    });

    const isVideo = format === 'mp4';
    const isAudio = format === 'mp3';
    refs.videoQualityBlock.style.display = isVideo ? 'block' : 'none';
    refs.audioQualityBlock.style.display = isAudio ? 'block' : 'none';
}

async function bootstrap() {
    try {
        setLoading(true);
        const [settings, stats, history, services, queue] = await Promise.all([
            fetchSettings(),
            fetchStats(),
            fetchHistory(),
            fetchServices(),
            fetchQueue(),
        ]);

        state.settings = settings;
        state.stats = stats;
        state.history = history;
        state.services = services.services ?? [];
        state.queue = queue;

        applySettings();
        renderStats();
        renderHistory();
        renderServices();
        renderQueue();
        startQueuePolling();
    } catch (error) {
        notifyError(error.message ?? 'Неизвестная ошибка');
    } finally {
        setLoading(false);
    }
}

function applySettings() {
    if (!state.settings) return;
    setSelectedFormat(state.settings.download_format ?? 'mp3');
    state.selectedVideoQuality = state.settings.video_quality ?? '720p';
    state.selectedAudioBitrate = state.settings.audio_bitrate ?? '320k';
    state.sendAsDocument = state.settings.send_as_document ?? false;
    state.sendAudioAsDocument = state.settings.send_audio_as_document ?? false;

    setActiveQuality(refs.videoQualityButtons, state.selectedVideoQuality, 'quality');
    setActiveQuality(refs.audioQualityButtons, state.selectedAudioBitrate, 'bitrate');

    refs.toggleVideoDoc.checked = state.sendAsDocument;
    refs.toggleAudioDoc.checked = state.sendAudioAsDocument;

    updatePlanUI(state.settings.plan ?? 'free');
    updatePlanButtons(state.settings.plan ?? 'free');
}

function setActiveQuality(buttons, value, attr) {
    let matched = false;
    buttons.forEach((btn) => {
        const key = attr === 'quality' ? btn.dataset.quality : btn.dataset.bitrate;
        const isActive = key === value;
        btn.classList.toggle('active', isActive);
        matched = matched || isActive;
    });

    if (!matched && buttons.length) {
        buttons[0].classList.add('active');
    }
}

function updatePlanUI(planKey) {
    const normalized = (planKey ?? 'free').toLowerCase();
    const config = PLAN_CONFIG[normalized] ?? PLAN_CONFIG.free;
    refs.planBadgeLabel.textContent = `${config.icon} ${config.label}`;
    refs.planLimits.textContent = config.limits;
    refs.requestQuota.textContent = config.quotaLabel;
    refs.profilePlan.textContent = `${config.icon} ${config.label}`;

    refs.planFeatures.innerHTML = config.features
        .map((feature) => `<li>${feature}</li>`)
        .join('');

    // Show cancel subscription button only for Premium and VIP
    if (normalized === 'premium' || normalized === 'vip') {
        refs.cancelSubscriptionBtn.style.display = 'block';
    } else {
        refs.cancelSubscriptionBtn.style.display = 'none';
    }

    updatePlanButtons(normalized);
}

function updatePlanButtons(activePlan) {
    refs.planButtons.forEach((btn) => {
        const plan = btn.dataset.planOption;
        const isActive = plan === activePlan;
        btn.classList.toggle('active', isActive);
        btn.disabled = state.planUpdateInProgress && !isActive;
    });
}

async function handlePlanChange(planKey) {
    state.planUpdateInProgress = true;
    setPlanButtonsLoading(planKey, true);
    updatePlanButtons(state.settings?.plan ?? planKey);
    try {
        await updateSettingsOnServer({ plan: planKey });
        if (state.settings) {
            state.settings.plan = planKey;
        }
        updatePlanUI(planKey);
        tg.HapticFeedback?.notificationOccurred?.('success');
    } catch (error) {
        notifyError(error.message);
    } finally {
        state.planUpdateInProgress = false;
        setPlanButtonsLoading(planKey, false);
        updatePlanButtons(state.settings?.plan ?? planKey);
    }
}

async function handleCancelSubscription() {
    const currentPlan = state.settings?.plan ?? 'free';
    if (currentPlan === 'free') {
        notifyError('У вас уже бесплатный план');
        return;
    }

    // Cancellation confirmation
    tg.showConfirm(
        `Отменить подписку ${PLAN_CONFIG[currentPlan]?.label ?? currentPlan}? Вы перейдете на бесплатный план.`,
        async (confirmed) => {
            if (confirmed) {
                await handlePlanChange('free');
                tg.showPopup({
                    title: '✅ Подписка отменена',
                    message: 'Вы перешли на бесплатный план. Все ваши данные сохранены.',
                    buttons: [{ type: 'ok' }],
                });
            }
        }
    );
}

function setPlanButtonsLoading(planKey, isLoading) {
    refs.planButtons.forEach((btn) => {
        if (btn.dataset.planOption === planKey) {
            btn.dataset.originalText = btn.dataset.originalText || btn.textContent;
            btn.textContent = isLoading ? '⏳ Обновляем...' : btn.dataset.originalText;
        } else if (!isLoading && btn.dataset.originalText) {
            btn.textContent = btn.dataset.originalText;
        }
    });
}

async function handlePreview() {
    const url = refs.urlInput.value.trim();
    if (!validateUrlOrWarn(url)) return;

    try {
        setPreviewLoading(true);
        const payload = {
            url,
            format: state.selectedFormat,
            video_quality: state.selectedFormat === 'mp4' ? state.selectedVideoQuality : undefined,
        };
        const preview = await apiFetch('/api/preview', {
            method: 'POST',
            body: JSON.stringify(payload),
        });
        state.preview = preview;
        renderPreview();
        tg.HapticFeedback?.notificationOccurred?.('success');
    } catch (error) {
        notifyError(error.message);
    } finally {
        setPreviewLoading(false);
    }
}

async function handleDownload() {
    const url = refs.urlInput.value.trim();
    if (!validateUrlOrWarn(url)) return;

    const payload = {
        url,
        format: state.selectedFormat,
        video_quality: state.selectedFormat === 'mp4' ? state.selectedVideoQuality : undefined,
        audio_bitrate: state.selectedFormat === 'mp3' ? state.selectedAudioBitrate : undefined,
        send_as_document: state.sendAsDocument,
        send_audio_as_document: state.sendAudioAsDocument,
    };

    try {
        setDownloadLoading(true);
        const response = await apiFetch('/api/download', {
            method: 'POST',
            body: JSON.stringify(payload),
        });

        tg.showPopup({
            title: '✅ Добавлено в очередь',
            message: `Позиция в очереди: ${response.queue_position}. Бот отправит файл, как только обработает.`,
            buttons: [{ type: 'ok' }],
        });

        refs.urlInput.value = '';
        state.preview = null;
        refs.previewCard.hidden = true;
        await Promise.all([refreshHistoryAndStats(), fetchQueue()]);
        tg.HapticFeedback?.notificationOccurred?.('success');
    } catch (error) {
        notifyError(error.message);
    } finally {
        setDownloadLoading(false);
    }
}

function validateUrlOrWarn(url) {
    if (!url) {
        notifyError('Вставь ссылку на видео или трек');
        return false;
    }

    if (!isValidUrl(url)) {
        notifyError('Некорректная ссылка. Проверь URL');
        return false;
    }

    return true;
}

function isValidUrl(value) {
    try {
        const parsed = new URL(value);
        return ['http:', 'https:'].includes(parsed.protocol);
    } catch {
        return false;
    }
}

function renderPreview() {
    if (!state.preview) return;
    refs.previewCard.hidden = false;
    refs.previewTitle.textContent = state.preview.title ?? 'Без названия';

    const duration = state.preview.duration_formatted ?? '—';
    const size = state.preview.filesize_formatted ?? '—';
    const formats =
        (state.preview.available_formats ?? [])
            .map((f) => f.toUpperCase())
            .join(', ') || '—';
    refs.previewMeta.innerHTML = `
        <span>${duration}</span>
        <span>${size}</span>
        <span>${formats}</span>
    `;
}

function renderStats() {
    if (!state.stats) return;
    const { total_downloads, successful_downloads, failed_downloads, total_size_bytes } = state.stats;

    refs.statsTotal.textContent = formatNumber(total_downloads);
    refs.statsSuccess.textContent = formatNumber(successful_downloads);
    refs.statsFailed.textContent = formatNumber(failed_downloads);
    refs.statsSize.textContent = formatBytes(total_size_bytes ?? 0);

    const successRate = total_downloads
        ? Math.round((successful_downloads / total_downloads) * 100)
        : 0;
    refs.successProgress.style.width = `${successRate}%`;
}

function renderHistory() {
    if (!state.history?.length) {
        refs.historyList.innerHTML = '';
        refs.historyEmpty.hidden = false;
        return;
    }

    refs.historyEmpty.hidden = true;
    refs.historyList.innerHTML = state.history
        .map((item) => {
            const statusIcon = item.status === 'completed' ? '✅' : item.status === 'failed' ? '⚠️' : '⏳';
            return `
                <div class="card history-item">
                    <div class="title">${statusIcon} ${item.title ?? item.url}</div>
                    <div class="history-meta">
                        <span>${item.format}</span>
                        <span>${formatDate(item.created_at)}</span>
                        ${item.error ? `<span>Ошибка: ${item.error}</span>` : ''}
                    </div>
                </div>
            `;
        })
        .join('');
}

function renderServices() {
    if (!state.services?.length) {
        refs.serviceList.innerHTML = '<li>Загрузка...</li>';
        return;
    }

    refs.serviceList.innerHTML = state.services
        .map((service) => `<li>${service.icon} ${service.name}</li>`)
        .join('');
}

function renderQueue() {
    if (!state.queue?.length) {
        refs.queueList.innerHTML = '';
        refs.queueEmpty.hidden = false;
        return;
    }

    refs.queueEmpty.hidden = true;

    // Sort: processing first, then pending by position
    const sortedQueue = [...state.queue].sort((a, b) => {
        if (a.status === 'processing' && b.status !== 'processing') return -1;
        if (a.status !== 'processing' && b.status === 'processing') return 1;
        return (a.queue_position || 0) - (b.queue_position || 0);
    });

    refs.queueList.innerHTML = sortedQueue
        .map((item) => {
            const statusLabel = formatQueueStatus(item.status);
            const isProcessing = item.status === 'processing';
            const positionLabel =
                item.status === 'pending'
                    ? `#${item.queue_position} в очереди`
                    : '⚙️ Обрабатывается';

            const progressBar = isProcessing
                ? '<div class="progress-bar" style="margin-top: 8px;"><span style="width: 60%; animation: pulse 2s infinite;"></span></div>'
                : '';

            return `
                <div class="queue-item" style="border-left: 3px solid ${isProcessing ? 'var(--tg-theme-button-color, #3390ec)' : 'rgba(255,255,255,0.1)'}; padding-left: 12px;">
                    <div class="title">${statusLabel} · ${item.format.toUpperCase()}</div>
                    <div class="queue-meta">
                        <span>${positionLabel}</span>
                        <span>${formatDate(item.created_at)}</span>
                        <span>${truncateUrl(item.url)}</span>
                    </div>
                    ${progressBar}
                </div>
            `;
        })
        .join('');
}

async function updateSettingsOnServer(payload) {
    try {
        await apiFetch(`/api/user/${state.userId}/settings`, {
            method: 'PATCH',
            body: JSON.stringify(payload),
        });
    } catch (error) {
        notifyError(error.message);
    }
}

async function refreshHistoryAndStats() {
    try {
        const [stats, history] = await Promise.all([fetchStats(), fetchHistory()]);
        state.stats = stats;
        state.history = history;
        renderStats();
        renderHistory();
    } catch (error) {
        console.error(error);
    }
}

async function fetchSettings() {
    return apiFetch(`/api/user/${state.userId}/settings`);
}

async function fetchStats() {
    return apiFetch(`/api/user/${state.userId}/stats`);
}

async function fetchHistory() {
    return apiFetch(`/api/user/${state.userId}/history?limit=10`);
}

async function fetchServices() {
    return apiFetch('/api/services');
}

async function fetchQueue() {
    try {
        // Show refresh indicator
        if (refs.queueUpdateIndicator) {
            refs.queueUpdateIndicator.style.opacity = '0.6';
        }

        const queue = await apiFetch(`/api/user/${state.userId}/queue`);
        state.queue = queue;
        renderQueue();

        // Hide refresh indicator
        setTimeout(() => {
            if (refs.queueUpdateIndicator) {
                refs.queueUpdateIndicator.style.opacity = '0';
            }
        }, 500);

        return queue;
    } catch (error) {
        console.error(error);
        if (refs.queueUpdateIndicator) {
            refs.queueUpdateIndicator.style.opacity = '0';
        }
        return [];
    }
}

function startQueuePolling() {
    stopQueuePolling();
    state.queuePollId = setInterval(fetchQueue, 5000);
}

function stopQueuePolling() {
    if (state.queuePollId) {
        clearInterval(state.queuePollId);
        state.queuePollId = null;
    }
}

async function apiFetch(path, options = {}) {
    const headers = new Headers(options.headers ?? {});
    if (!headers.has('Content-Type') && options.body) {
        headers.set('Content-Type', 'application/json');
    }

    if (!headers.has('X-Telegram-Init-Data') && tg.initData) {
        headers.set('X-Telegram-Init-Data', tg.initData);
    }

    const response = await fetch(path, {
        credentials: 'same-origin',
        ...options,
        headers,
    });

    let data = null;
    try {
        data = await response.json();
    } catch {
        data = null;
    }

    if (!response.ok) {
        const message = data?.error ?? 'Запрос завершился с ошибкой';
        throw new Error(message);
    }

    return data ?? {};
}

function setPreviewLoading(isLoading) {
    refs.previewBtn.disabled = isLoading;
    refs.previewBtn.textContent = isLoading ? '⏳ Получаем превью...' : '👀 Получить превью';
}

function setDownloadLoading(isLoading) {
    state.loading = isLoading;
    refs.downloadBtn.disabled = isLoading;
    tg.MainButton.setParams({ is_active: !isLoading });
    refs.downloadBtn.textContent = isLoading ? '⏳ Обработка...' : '📥 Скачать';
}

function setLoading(isLoading) {
    state.loading = isLoading;
    refs.downloadBtn.disabled = isLoading;
    refs.previewBtn.disabled = isLoading;
    tg.MainButton.setParams({ is_active: !isLoading });
}

function notifyError(message) {
    tg.showAlert(message);
    tg.HapticFeedback?.notificationOccurred?.('error');
}

function formatQueueStatus(status) {
    switch (status) {
        case 'processing':
            return '⚙️ Обработка';
        case 'pending':
            return '⏳ В очереди';
        default:
            return status;
    }
}

function formatBytes(bytes) {
    if (!bytes) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const index = Math.floor(Math.log(bytes) / Math.log(1024));
    const value = bytes / 1024 ** index;
    return `${value.toFixed(1)} ${units[index]}`;
}

function formatNumber(value) {
    return new Intl.NumberFormat('ru-RU').format(value ?? 0);
}

function formatDate(value) {
    if (!value) return '—';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return date.toLocaleString('ru-RU', { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit' });
}

function truncateUrl(url, max = 28) {
    if (!url) return '';
    if (url.length <= max) return url;
    return `${url.slice(0, max - 3)}...`;
}

async function loadAdminData() {
    try {
        // Load admin statistics
        const adminStats = await apiFetch('/api/admin/stats');
        renderAdminStats(adminStats);
    } catch (error) {
        console.error('Ошибка загрузки данных администратора:', error);
        notifyError('Не удалось загрузить административные данные');
    }
}

function renderAdminStats(stats) {
    if (!stats) return;

    // Primary stats
    refs.adminTotalUsers.textContent = formatNumber(stats.total_users ?? 0);
    refs.adminTotalDownloads.textContent = formatNumber(stats.total_downloads ?? 0);
    refs.adminActiveQueue.textContent = formatNumber(stats.active_queue ?? 0);
    refs.adminTotalSize.textContent = formatBytes(stats.total_size ?? 0);

    // Plan distribution
    refs.adminPlanFree.textContent = formatNumber(stats.plans?.free ?? 0);
    refs.adminPlanPremium.textContent = formatNumber(stats.plans?.premium ?? 0);
    refs.adminPlanVip.textContent = formatNumber(stats.plans?.vip ?? 0);

    // Queue (when data is present)
    if (stats.queue && stats.queue.length > 0) {
        refs.adminQueueEmpty.hidden = true;
        refs.adminQueueList.innerHTML = stats.queue
            .map((item) => {
                const statusLabel = formatQueueStatus(item.status);
                const isProcessing = item.status === 'processing';

                return `
                    <div class="queue-item" style="border-left: 3px solid ${isProcessing ? 'var(--tg-theme-button-color, #3390ec)' : 'rgba(255,255,255,0.1)'}; padding-left: 12px;">
                        <div class="title">${statusLabel} · ${item.format?.toUpperCase() ?? '—'}</div>
                        <div class="queue-meta">
                            <span>User: ${item.user_id}</span>
                            <span>${formatDate(item.created_at)}</span>
                            <span>${truncateUrl(item.url)}</span>
                        </div>
                    </div>
                `;
            })
            .join('');
    } else {
        refs.adminQueueEmpty.hidden = false;
        refs.adminQueueList.innerHTML = '';
    }
}

init();
