import i18n from './i18n.js?v=15';

// Globals
let tokenChartInstance = null;
let monthlyChartInstance = null;

const chartPalette = {
  tokenFill: 'rgba(47, 184, 197, 0.24)',
  tokenStroke: '#2fb8c5',
  cacheFill: 'rgba(137, 151, 172, 0.62)',
  cacheStroke: '#8997ac',
  trendFill: 'rgba(246, 190, 79, 0.14)',
  trendStroke: '#f6be4f',
};

const chartFontFamily = 'IBM Plex Sans';
const SIDEBAR_STATE_STORAGE_KEY = 'sidebar_state';
const utf8TextEncoder = new TextEncoder();

// Cookie helper functions
function setCookie(name, value, days = 365) {
  const date = new Date();
  date.setTime(date.getTime() + (days * 24 * 60 * 60 * 1000));
  const expires = "; expires=" + date.toUTCString();
  document.cookie = name + "=" + encodeURIComponent(value) + expires + "; path=/; SameSite=Strict";
}

function getCookie(name) {
  const nameEQ = name + "=";
  const ca = document.cookie.split(';');
  for(let i = 0; i < ca.length; i++) {
    let c = ca[i];
    while (c.charAt(0) == ' ') c = c.substring(1, c.length);
    if (c.indexOf(nameEQ) == 0) return decodeURIComponent(c.substring(nameEQ.length, c.length));
  }
  return null;
}

const assistantAliasMap = {
  'claude-code': 'claude',
  'claude_code': 'claude',
  'claudecode': 'claude',
};

const assistantMeta = {
  antigravity: {
    logo: '/static/antigravity.webp',
    label: 'Antigravity CLI',
    shortLabel: 'Antigravity',
    alt: 'Antigravity',
    badgeStyle: 'background: rgba(47, 184, 197, 0.13); color: #2fb8c5; border: 1px solid rgba(47, 184, 197, 0.26); display: inline-flex; align-items: center;',
    senderName: 'ANTIGRAVITY AGENT',
  },
  copilot: {
    logo: '/static/githubcopilot.webp',
    label: 'GitHub Copilot CLI',
    shortLabel: 'Copilot',
    alt: 'Copilot',
    badgeStyle: 'background: rgba(185, 43, 39, 0.15); color: #b92b27; border: 1px solid rgba(185, 43, 39, 0.3); display: inline-flex; align-items: center;',
    senderName: 'COPILOT AGENT',
  },
  codex: {
    logo: '/static/codex.webp',
    label: 'Codex CLI',
    shortLabel: 'Codex',
    alt: 'Codex',
    badgeStyle: 'background: rgba(16, 185, 129, 0.15); color: #10b981; border: 1px solid rgba(16, 185, 129, 0.3); display: inline-flex; align-items: center;',
    senderName: 'CODEX AGENT',
  },
  claude: {
    logo: '/static/claude-code-logo.svg',
    label: 'Claude Code',
    shortLabel: 'Claude Code',
    alt: 'Claude Code',
    badgeStyle: 'background: rgba(79, 126, 168, 0.15); color: #7aa7cf; border: 1px solid rgba(79, 126, 168, 0.3); display: inline-flex; align-items: center;',
    senderName: 'CLAUDE CODE AGENT',
  },
};

function normalizeAssistant(rawValue) {
  const normalized = String(rawValue || '').trim().toLowerCase();
  return assistantAliasMap[normalized] || normalized;
}

function isSupportedAssistant(rawValue) {
  const normalized = normalizeAssistant(rawValue);
  return Object.prototype.hasOwnProperty.call(assistantMeta, normalized);
}

function getAssistantMeta(rawValue) {
  const normalized = normalizeAssistant(rawValue);
  return assistantMeta[normalized] || {
    logo: '/static/favicon-v2.png',
    label: normalized || 'Agent',
    shortLabel: normalized || 'Agent',
    alt: normalized || 'Agent',
    badgeStyle: 'display: inline-flex; align-items: center;',
    senderName: 'AGENT',
  };
}

function getAssistantLogoHtml(rawValue, className = 'badge-logo') {
  const meta = getAssistantMeta(rawValue);
  return `<img class="${className}" src="${meta.logo}" alt="${meta.alt}" />`;
}

function getUrlDateForTab(tab) {
  const urlParams = new URLSearchParams(window.location.search);
  const urlDate = urlParams.get('date');
  if (!urlDate) return null;
  
  if (tab === 'daily') {
    return urlDate;
  } else if (tab === 'monthly') {
    if (/^\d{4}-\d{2}-\d{2}$/.test(urlDate)) {
      return urlDate.substring(0, 7);
    }
    return urlDate;
  } else if (tab === 'yearly') {
    if (/^\d{4}-\d{2}(-\d{2})?$/.test(urlDate)) {
      return urlDate.substring(0, 4);
    }
    return urlDate;
  }
  return urlDate;
}

function updateUrlParams() {
  const url = new URL(window.location.href);
  url.searchParams.set('agent', currentAssistant);
  url.searchParams.set('tab', activeTab);
  
  if (activeTab === 'daily') {
    const dateSelect = document.getElementById('date-select');
    if (dateSelect && dateSelect.value) {
      url.searchParams.set('date', dateSelect.value);
    }
  } else if (activeTab === 'monthly') {
    const monthSelect = document.getElementById('month-select');
    if (monthSelect && monthSelect.value) {
      url.searchParams.set('date', monthSelect.value);
    }
  } else if (activeTab === 'yearly') {
    const yearSelect = document.getElementById('year-select');
    if (yearSelect && yearSelect.value) {
      url.searchParams.set('date', yearSelect.value);
    }
  }
  
  window.history.replaceState(null, '', url.toString());
}

const urlParams = new URLSearchParams(window.location.search);
const urlAgent = urlParams.get('agent');
const savedAgent = isSupportedAssistant(urlAgent) ? urlAgent : getCookie('selected_agent');
let currentAssistant = isSupportedAssistant(savedAgent) ? normalizeAssistant(savedAgent) : 'antigravity';

const urlTab = urlParams.get('tab');
const savedTab = ['daily', 'monthly', 'yearly'].includes(urlTab) ? urlTab : getCookie('active_tab');
let activeTab = ['daily', 'monthly', 'yearly'].includes(savedTab) ? savedTab : 'daily'; // 'daily' or 'monthly' or 'yearly'
let isEmptyState = false;
let currentChartSessions = [];
let currentMonthlyBreakdown = [];
let currentSessionTotalTokens = 0;
let currentSessionCacheTokens = 0;
let currentSessionInputTokens = 0;
let currentSessionOutputTokens = 0;
let currentSessionReasoningTokens = 0;
let currentSessionCwd = '';
let currentSessionModel = '';
let currentSessionAssistantType = '';
let availableDates = [];
let pricingRules = [];

function getLocalDateString(date = new Date()) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function renderSafeMarkdown(markdownText) {
  const rawText = markdownText || '';
  const parsedHtml = typeof marked === 'undefined'
    ? escapeHtml(String(rawText))
    : marked.parse(String(rawText));

  if (typeof DOMPurify === 'undefined') {
    return escapeHtml(String(rawText));
  }

  return DOMPurify.sanitize(parsedHtml);
}

// Session table sorting state
let currentSessions = [];
let currentSortColumn = 'timestamp'; // Default sorted by starting time
let currentSortDirection = 'desc';  // Default chronological order

// Monthly daily summary sorting state
let monthlyDailySortColumn = 'date';
let monthlyDailySortDirection = 'desc';
let currentMonthlyChartData = [];

// Yearly monthly summary sorting state
let yearlyMonthlySortColumn = 'month';
let yearlyMonthlySortDirection = 'desc';
let currentYearlyBreakdown = [];
let currentYearlyData = null;
let yearlyChartInstance = null;
let currentYearlyChartData = [];

// Live Auto-Refresh State
let liveRefreshTimer = null;
let liveProgressTimer = null;
let secondsRemaining = 10;
let refreshInterval = 10000; // default 10s

// Language / Internationalization (i18n) State
let currentLang = localStorage.getItem('lang') || 'zh-TW';
let currentUsageData = null;
let currentMonthlyData = null;
let cachedCodexResets = null;
let isQueryingCodexResets = false;

// i18n localization dictionary is now loaded from /static/i18n.js

function t(key) {
  const isSingle = isSupportedAssistant(currentAssistant);
  if (isSingle) {
    const prefix = currentAssistant + '_';
    return i18n[currentLang][prefix + key] || i18n[currentLang][key] || i18n['zh-TW'][prefix + key] || i18n['zh-TW'][key] || key;
  }
  return i18n[currentLang][key] || i18n['zh-TW'][key] || key;
}

function iconMarkup(name, extraClass = '') {
  const classes = ['icon-glyph', `icon-${name}`, extraClass].filter(Boolean).join(' ');
  return `<span class="${classes}" aria-hidden="true"></span>`;
}

function cardIconMarkup(name, extraClass = '') {
  const classes = ['card-icon', extraClass].filter(Boolean).join(' ');
  return `<div class="${classes}">${iconMarkup(name)}</div>`;
}

function setTitleMarkup(iconName, textHtml) {
  const titleEl = document.getElementById('current-date-title');
  if (titleEl) {
    titleEl.innerHTML = `<span class="title-text">${textHtml}</span>`;
  }
}

function setDisclosureIcon(target, expanded) {
  if (target) {
    target.innerHTML = iconMarkup(expanded ? 'chevron-up' : 'chevron-down', 'toggle-arrow-icon');
  }
}

function updateBrandLogo() {
  const brandLogo = document.getElementById('brand-logo-img');
  if (!brandLogo) return;

  const meta = getAssistantMeta(currentAssistant);
  brandLogo.src = meta.logo;
  brandLogo.alt = meta.alt;
}

function languageMeta(lang) {
  return lang === 'en'
    ? { flag: '🇺🇸', label: '美國', next: 'zh-TW' }
    : { flag: '🇹🇼', label: '臺灣', next: 'en' };
}

function updateLanguageToggle() {
  const langToggle = document.getElementById('lang-toggle-btn');
  if (!langToggle) return;

  const meta = languageMeta(currentLang);
  const label = currentLang === 'en'
    ? `Switch language, current: United States`
    : `切換語言，目前：${meta.label}`;
  langToggle.textContent = meta.flag;
  langToggle.title = label;
  langToggle.setAttribute('aria-label', label);
}

function syncSidebarToggleButton() {
  const sidebarToggleBtn = document.getElementById('sidebar-toggle-btn');
  const appContainer = document.querySelector('.app-container');
  if (!sidebarToggleBtn || !appContainer) return;

  const isCollapsed = appContainer.classList.contains('sidebar-collapsed');
  const label = t(isCollapsed ? 'sidebar_toggle_open' : 'sidebar_toggle_collapse');
  sidebarToggleBtn.classList.toggle('is-collapsed', isCollapsed);
  sidebarToggleBtn.title = label;
  sidebarToggleBtn.setAttribute('aria-label', label);
  sidebarToggleBtn.setAttribute('aria-expanded', String(!isCollapsed));
}

function getSavedSidebarState() {
  const state = localStorage.getItem(SIDEBAR_STATE_STORAGE_KEY);
  return state === 'collapsed' || state === 'expanded' ? state : null;
}

function saveSidebarState(isCollapsed) {
  localStorage.setItem(SIDEBAR_STATE_STORAGE_KEY, isCollapsed ? 'collapsed' : 'expanded');
}

function setSidebarCollapsed(isCollapsed, { persist = false } = {}) {
  const appContainer = document.querySelector('.app-container');
  if (!appContainer) return;

  appContainer.classList.toggle('sidebar-collapsed', isCollapsed);
  if (persist) {
    saveSidebarState(isCollapsed);
  }
  syncSidebarToggleButton();
}

function applyInitialSidebarState() {
  const savedState = getSavedSidebarState();
  const shouldCollapse = savedState
    ? savedState === 'collapsed'
    : window.innerWidth <= 1024;
  setSidebarCollapsed(shouldCollapse);
}

function isEditableShortcutTarget(target) {
  if (!(target instanceof HTMLElement)) return false;
  const tagName = target.tagName.toLowerCase();
  return tagName === 'input'
    || tagName === 'textarea'
    || tagName === 'select'
    || target.isContentEditable;
}

function toggleSidebar() {
  const appContainer = document.querySelector('.app-container');
  if (!appContainer) return;
  setSidebarCollapsed(!appContainer.classList.contains('sidebar-collapsed'), { persist: true });
}

function updateLanguageUI() {
  document.title = 'Token 戰情室';

  document.querySelectorAll('[data-i18n]').forEach(el => {
    const key = el.getAttribute('data-i18n');
    el.innerHTML = t(key);
  });

  document.querySelectorAll('[data-i18n-title]').forEach(el => {
    const key = el.getAttribute('data-i18n-title');
    const label = t(key);
    el.title = label;
    if (el.hasAttribute('aria-label')) {
      el.setAttribute('aria-label', label);
    }
  });

  document.querySelectorAll('[data-i18n-placeholder]').forEach(el => {
    const key = el.getAttribute('data-i18n-placeholder');
    el.placeholder = t(key);
  });

  // Specific dynamic text updates
  updateLanguageToggle();

  const themeBtn = document.getElementById('theme-toggle-btn');
  if (themeBtn) {
    const currentTheme = document.documentElement.getAttribute('data-theme') || 'dark';
    const title = currentTheme === 'dark' ? t('theme_toggle_title_dark') : t('theme_toggle_title_light');
    themeBtn.title = title;
    themeBtn.setAttribute('aria-label', title);
  }

  // Update dynamic placeholders/empty state if they are currently displayed
  const emptyContainer = document.getElementById('empty-state-container');
  if (emptyContainer && !emptyContainer.classList.contains('hidden')) {
    toggleEmptyState(true);
  }

  // Update dynamic brand logo in sidebar
  updateBrandLogo();
  syncSidebarToggleButton();
  updateCodexRateLimit();
}

document.addEventListener('DOMContentLoaded', () => {
  initApp();
});

// =========================================================================
// App Initialization & Event Listeners
// =========================================================================
function initApp() {
  const dateSelect = document.getElementById('date-select');
  const monthSelect = document.getElementById('month-select');
  const yearSelect = document.getElementById('year-select');
  const closeDrawerBtn = document.getElementById('close-drawer-btn');
  const drawerOverlay = document.getElementById('timeline-drawer');

  // Tab Buttons
  const tabBtnDaily = document.getElementById('tab-btn-daily');
  const tabBtnMonthly = document.getElementById('tab-btn-monthly');
  const tabBtnYearly = document.getElementById('tab-btn-yearly');

  // Live Controls
  const liveToggle = document.getElementById('live-toggle');
  const liveInterval = document.getElementById('live-interval');

  // Apply initial tab visibility based on restored activeTab
  const dailySelector = document.getElementById('daily-selector-section');
  const monthlySelector = document.getElementById('monthly-selector-section');
  const yearlySelector = document.getElementById('yearly-selector-section');
  const quickStats = document.getElementById('quick-stats-section');
  const dailyView = document.getElementById('daily-view-container');
  const monthlyView = document.getElementById('monthly-view-container');
  const yearlyView = document.getElementById('yearly-view-container');

  if (activeTab === 'daily') {
    tabBtnDaily.classList.add('active');
    tabBtnMonthly.classList.remove('active');
    if (tabBtnYearly) tabBtnYearly.classList.remove('active');
    dailySelector.classList.remove('hidden');
    monthlySelector.classList.add('hidden');
    if (yearlySelector) yearlySelector.classList.add('hidden');
    if (quickStats) quickStats.classList.remove('hidden');
    
    if (dailyView) dailyView.classList.remove('hidden');
    if (monthlyView) monthlyView.classList.add('hidden');
    if (yearlyView) yearlyView.classList.add('hidden');
  } else if (activeTab === 'monthly') {
    tabBtnDaily.classList.remove('active');
    tabBtnMonthly.classList.add('active');
    if (tabBtnYearly) tabBtnYearly.classList.remove('active');
    dailySelector.classList.add('hidden');
    monthlySelector.classList.remove('hidden');
    if (yearlySelector) yearlySelector.classList.add('hidden');
    if (quickStats) quickStats.classList.add('hidden');
    
    if (dailyView) dailyView.classList.add('hidden');
    if (monthlyView) monthlyView.classList.remove('hidden');
    if (yearlyView) yearlyView.classList.add('hidden');
  } else if (activeTab === 'yearly') {
    tabBtnDaily.classList.remove('active');
    tabBtnMonthly.classList.remove('active');
    if (tabBtnYearly) tabBtnYearly.classList.add('active');
    dailySelector.classList.add('hidden');
    monthlySelector.classList.add('hidden');
    if (yearlySelector) yearlySelector.classList.remove('hidden');
    if (quickStats) quickStats.classList.add('hidden');
    
    if (dailyView) dailyView.classList.add('hidden');
    if (monthlyView) monthlyView.classList.add('hidden');
    if (yearlyView) yearlyView.classList.remove('hidden');
  }

  // Set initial live refresh toggle checkbox state based on cookie
  const savedLiveRefresh = getCookie('live_refresh') === 'true';
  if (liveToggle) {
    if (savedLiveRefresh && activeTab === 'daily') {
      liveToggle.checked = true;
    } else {
      liveToggle.checked = false;
    }
  }

  // 監聽助理切換 (單選 Badge)
  const badgeButtons = document.querySelectorAll('.assistant-badge-btn');
  if (badgeButtons.length > 0) {
    // 初始化：找到第一個符合 currentAssistant 的按鈕，或預設第一個
    badgeButtons.forEach(btn => {
      const val = btn.getAttribute('data-value');
      if (normalizeAssistant(val) === currentAssistant) {
        btn.classList.add('active');
      } else {
        btn.classList.remove('active');
      }
    });
    // 若沒有任何 active（例如 currentAssistant === 'all' 或 'none'），預設第一個
    if (!document.querySelector('.assistant-badge-btn.active')) {
      badgeButtons[0].classList.add('active');
      currentAssistant = badgeButtons[0].getAttribute('data-value');
      currentAssistant = normalizeAssistant(currentAssistant);
      setCookie('selected_agent', currentAssistant);
    }

    badgeButtons.forEach(btn => {
      btn.addEventListener('click', async () => {
        // 單選：先取消所有，再啟用此按鈕
        badgeButtons.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');

        currentAssistant = normalizeAssistant(btn.getAttribute('data-value'));
        setCookie('selected_agent', currentAssistant);
        updateUrlParams();
        
        updateLanguageUI();
        fetchPricingRules();

        const colHeader = document.getElementById('col-assistant-header');
        if (colHeader) {
          colHeader.classList.add('hidden');
        }

        // 切換 agent 時保留目前日期，當日無資料則顯示提示
        await fetchDates(null, true);
        await fetchMonths();
        await fetchYears();
      });
    });
  }

  // Language toggle
  const langToggle = document.getElementById('lang-toggle-btn');
  if (langToggle) {
    updateLanguageToggle();
    langToggle.addEventListener('click', () => {
      currentLang = languageMeta(currentLang).next;
      localStorage.setItem('lang', currentLang);
      updateLanguageUI();
      
      // Re-render currently active view
      if (activeTab === 'daily' && currentUsageData) {
        renderDashboard(currentUsageData);
      } else if (activeTab === 'monthly' && currentMonthlyData) {
        renderMonthlyDashboard(currentMonthlyData);
      } else if (activeTab === 'yearly' && currentYearlyData) {
        renderYearlyDashboard(currentYearlyData);
      }
    });
  }

  // 載入日期清單
  fetchDates();
  // 載入月份清單
  fetchMonths();
  // 載入年份清單
  fetchYears();

  // Initialize and synchronize custom hover-dropdown selectors
  const tabHoverOpts = document.querySelectorAll('.tab-hover-dropdown .hover-dropdown-option');
  const tabLabelEls = document.querySelectorAll('.tab-dropdown-label');

  if (tabLabelEls.length > 0) {
    tabLabelEls.forEach(labelEl => {
      const activeTabOpt = document.querySelector(`.tab-hover-dropdown .hover-dropdown-option[data-value="${activeTab}"]`);
      if (activeTabOpt) {
        labelEl.setAttribute('data-i18n', activeTabOpt.getAttribute('data-i18n'));
      }
    });
  }
  tabHoverOpts.forEach(opt => {
    if (opt.getAttribute('data-value') === activeTab) {
      opt.classList.add('active');
    } else {
      opt.classList.remove('active');
    }
    opt.addEventListener('click', () => {
      const val = opt.getAttribute('data-value');
      const btn = document.getElementById(`tab-btn-${val}`);
      if (btn) btn.click();
    });
  });

  // Initialize language UI translation
  updateLanguageUI();

  // Tab切換監聽
  tabBtnDaily.addEventListener('click', () => switchTab('daily'));
  tabBtnMonthly.addEventListener('click', () => switchTab('monthly'));
  tabBtnYearly.addEventListener('click', () => switchTab('yearly'));

  // 監聽日期切換
  dateSelect.addEventListener('change', (e) => {
    if (e.target.value) {
      loadUsageData(e.target.value);
    }
  });

  // 點擊整個輸入框時自動打開小日曆
  dateSelect.addEventListener('click', (e) => {
    if (typeof e.target.showPicker === 'function') {
      try {
        e.target.showPicker();
      } catch (err) {
        console.warn('showPicker not supported or blocked:', err);
      }
    }
  });

  // 快速切換前一天與後一天邏輯
  const adjustDate = async (offset) => {
    const currentDateVal = dateSelect.value;
    if (!currentDateVal) return;
    
    const currentDate = new Date(currentDateVal);
    if (isNaN(currentDate.getTime())) return;
    
    currentDate.setDate(currentDate.getDate() + offset);
    const newDateStr = getLocalDateString(currentDate);
    dateSelect.value = newDateStr;
    await loadUsageData(newDateStr);
  };

  const btnPrevDay = document.getElementById('btn-prev-day');
  if (btnPrevDay) {
    btnPrevDay.addEventListener('click', () => adjustDate(-1));
  }

  const btnNextDay = document.getElementById('btn-next-day');
  if (btnNextDay) {
    btnNextDay.addEventListener('click', () => adjustDate(1));
  }

  // 監聽今日按鈕
  const btnToday = document.getElementById('btn-today');
  if (btnToday) {
    btnToday.addEventListener('click', async () => {
      const todayStr = getLocalDateString();
      if (dateSelect) {
        dateSelect.value = todayStr;
      }
      await loadUsageData(todayStr);
      showNotification(`${t('today_btn') || '今日'} ${todayStr}`, 'success');
    });
  }

  // 監聽月份切換
  monthSelect.addEventListener('change', (e) => {
    if (e.target.value) {
      loadMonthlyData(e.target.value);
    }
  });

  // 快速切換上個月與下個月邏輯
  const adjustMonth = async (offset) => {
    const currentMonthVal = monthSelect.value;
    if (!currentMonthVal) return;
    
    const parts = currentMonthVal.split('-');
    if (parts.length !== 2) return;
    const year = parseInt(parts[0], 10);
    const month = parseInt(parts[1], 10);
    
    let targetMonth = month - 1 + offset;
    let targetYear = year + Math.floor(targetMonth / 12);
    targetMonth = (targetMonth % 12 + 12) % 12 + 1;
    
    const newMonthStr = `${targetYear}-${String(targetMonth).padStart(2, '0')}`;
    
    let exists = false;
    for (let i = 0; i < monthSelect.options.length; i++) {
      if (monthSelect.options[i].value === newMonthStr) {
        exists = true;
        break;
      }
    }
    if (!exists) {
      const opt = document.createElement('option');
      opt.value = newMonthStr;
      opt.textContent = newMonthStr;
      let inserted = false;
      for (let i = 0; i < monthSelect.options.length; i++) {
        if (monthSelect.options[i].value < newMonthStr) {
          monthSelect.insertBefore(opt, monthSelect.options[i]);
          inserted = true;
          break;
        }
      }
      if (!inserted) {
        monthSelect.appendChild(opt);
      }
    }
    
    monthSelect.value = newMonthStr;
    await loadMonthlyData(newMonthStr);
  };

  const btnPrevMonth = document.getElementById('btn-prev-month');
  if (btnPrevMonth) {
    btnPrevMonth.addEventListener('click', () => adjustMonth(-1));
  }

  const btnNextMonth = document.getElementById('btn-next-month');
  if (btnNextMonth) {
    btnNextMonth.addEventListener('click', () => adjustMonth(1));
  }

  const btnThisMonth = document.getElementById('btn-this-month');
  if (btnThisMonth) {
    btnThisMonth.addEventListener('click', async () => {
      const now = new Date();
      const thisMonthStr = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
      
      let exists = false;
      for (let i = 0; i < monthSelect.options.length; i++) {
        if (monthSelect.options[i].value === thisMonthStr) {
          exists = true;
          break;
        }
      }
      if (!exists) {
        const opt = document.createElement('option');
        opt.value = thisMonthStr;
        opt.textContent = thisMonthStr;
        let inserted = false;
        for (let i = 0; i < monthSelect.options.length; i++) {
          if (monthSelect.options[i].value < thisMonthStr) {
            monthSelect.insertBefore(opt, monthSelect.options[i]);
            inserted = true;
            break;
          }
        }
        if (!inserted) {
          monthSelect.appendChild(opt);
        }
      }
      monthSelect.value = thisMonthStr;
      await loadMonthlyData(thisMonthStr);
      showNotification(`${t('this_month_btn') || '今月'} ${thisMonthStr}`, 'success');
    });
  }

  // 監聽重新整理按鈕
  const btnReloadDaily = document.getElementById('btn-reload-daily');
  const btnReloadMonthly = document.getElementById('btn-reload-monthly');

  if (btnReloadDaily) {
    btnReloadDaily.addEventListener('click', async () => {
      btnReloadDaily.classList.add('loading');
      try {
        await reloadDailyData();
        showNotification(t('reload_success'), 'success');
      } catch (err) {
        console.error('Reload failed:', err);
        showNotification(t('reload_failed'), 'error');
      } finally {
        btnReloadDaily.classList.remove('loading');
      }
    });
  }

  if (btnReloadMonthly) {
    btnReloadMonthly.addEventListener('click', async () => {
      btnReloadMonthly.classList.add('loading');
      try {
        await reloadMonthlyData();
        showNotification(t('monthly_reload_success'), 'success');
      } catch (err) {
        console.error('Reload failed:', err);
        showNotification(t('reload_failed'), 'error');
      } finally {
        btnReloadMonthly.classList.remove('loading');
      }
    });
  }

  // 監聽年份切換
  if (yearSelect) {
    yearSelect.addEventListener('change', (e) => {
      if (e.target.value) {
        loadYearlyData(e.target.value);
      }
    });
  }

  // 快速切換上一年與下一年邏輯
  const adjustYear = async (offset) => {
    if (!yearSelect) return;
    const currentYearVal = yearSelect.value;
    if (!currentYearVal) return;
    
    const year = parseInt(currentYearVal, 10);
    const targetYear = year + offset;
    const newYearStr = String(targetYear);
    
    let exists = false;
    for (let i = 0; i < yearSelect.options.length; i++) {
      if (yearSelect.options[i].value === newYearStr) {
        exists = true;
        break;
      }
    }
    if (!exists) {
      const opt = document.createElement('option');
      opt.value = newYearStr;
      opt.textContent = newYearStr;
      let inserted = false;
      for (let i = 0; i < yearSelect.options.length; i++) {
        if (yearSelect.options[i].value < newYearStr) {
          yearSelect.insertBefore(opt, yearSelect.options[i]);
          inserted = true;
          break;
        }
      }
      if (!inserted) {
        yearSelect.appendChild(opt);
      }
    }
    
    yearSelect.value = newYearStr;
    await loadYearlyData(newYearStr);
  };

  const btnPrevYear = document.getElementById('btn-prev-year');
  if (btnPrevYear) {
    btnPrevYear.addEventListener('click', () => adjustYear(-1));
  }

  const btnNextYear = document.getElementById('btn-next-year');
  if (btnNextYear) {
    btnNextYear.addEventListener('click', () => adjustYear(1));
  }

  const btnThisYear = document.getElementById('btn-this-year');
  if (btnThisYear) {
    btnThisYear.addEventListener('click', async () => {
      if (!yearSelect) return;
      const now = new Date();
      const thisYearStr = String(now.getFullYear());
      
      let exists = false;
      for (let i = 0; i < yearSelect.options.length; i++) {
        if (yearSelect.options[i].value === thisYearStr) {
          exists = true;
          break;
        }
      }
      if (!exists) {
        const opt = document.createElement('option');
        opt.value = thisYearStr;
        opt.textContent = thisYearStr;
        let inserted = false;
        for (let i = 0; i < yearSelect.options.length; i++) {
          if (yearSelect.options[i].value < thisYearStr) {
            yearSelect.insertBefore(opt, yearSelect.options[i]);
            inserted = true;
            break;
          }
        }
        if (!inserted) {
          yearSelect.appendChild(opt);
        }
      }
      yearSelect.value = thisYearStr;
      await loadYearlyData(thisYearStr);
      showNotification(`${t('this_year_btn') || '今年'} ${thisYearStr}`, 'success');
    });
  }

  const btnReloadYearly = document.getElementById('btn-reload-yearly');
  if (btnReloadYearly) {
    btnReloadYearly.addEventListener('click', async () => {
      btnReloadYearly.classList.add('loading');
      try {
        await reloadYearlyData();
        showNotification(t('yearly_reload_success'), 'success');
      } catch (err) {
        console.error('Reload failed:', err);
        showNotification(t('reload_failed'), 'error');
      } finally {
        btnReloadYearly.classList.remove('loading');
      }
    });
  }

  // 監聽手動同步資料庫按鈕
  const btnSyncDb = document.getElementById('btn-sync-db');
  if (btnSyncDb) {
    btnSyncDb.addEventListener('click', async () => {
      btnSyncDb.classList.add('loading');
      btnSyncDb.disabled = true;
      showNotification(t('sync_db_loading'), 'info');
      try {
        const res = await fetch(`/api/${currentAssistant}/sync`);
        if (res.ok) {
          showNotification(t('sync_db_success'), 'success');
          // 重新載入目前頁面的數據
          if (activeTab === 'daily') {
            await reloadDailyData();
          } else if (activeTab === 'monthly') {
            await reloadMonthlyData();
          } else if (activeTab === 'yearly') {
            await reloadYearlyData();
          }
          // 同時重新整理可用的日期、月份與年份清單
          await fetchDates();
          await fetchMonths();
          await fetchYears();
        } else {
          let errMsg = res.statusText;
          try {
            const data = await res.json();
            if (data && data.error) errMsg = data.error;
          } catch (_) {}
          showNotification(t('sync_db_failed') + errMsg, 'error');
        }
      } catch (err) {
        console.error('Sync failed:', err);
        showNotification(t('sync_db_failed') + err.message, 'error');
      } finally {
        btnSyncDb.classList.remove('loading');
        btnSyncDb.disabled = false;
      }
    });
  }

  // 監聽 Live 重新整理切換
  liveToggle.addEventListener('change', (e) => {
    toggleLiveRefresh(e.target.checked);
    setCookie('live_refresh', e.target.checked ? 'true' : 'false');
  });

  // 監聽 Live 頻率變更
  liveInterval.addEventListener('change', (e) => {
    refreshInterval = parseInt(e.target.value, 10);
    if (liveToggle.checked) {
      // 重啟計時器
      startLiveRefresh();
    }
  });

  // 關閉抽屜彈窗
  closeDrawerBtn.addEventListener('click', closeDrawer);
  drawerOverlay.addEventListener('click', (e) => {
    if (e.target === drawerOverlay) {
      closeDrawer();
    }
  });

  // 支援 ESC 鍵關閉抽屜與關閉行動端側欄
  window.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'b' && !isEditableShortcutTarget(e.target)) {
      e.preventDefault();
      toggleSidebar();
      return;
    }

    if (e.key === 'Escape') {
      closeDrawer();
      const container = document.querySelector('.app-container');
      if (container && window.innerWidth <= 992) {
        setSidebarCollapsed(true, { persist: true });
      }
    }
  });

  // Sidebar Toggle Button
  const sidebarToggleBtn = document.getElementById('sidebar-toggle-btn');
  const appContainer = document.querySelector('.app-container');
  if (sidebarToggleBtn && appContainer) {
    sidebarToggleBtn.addEventListener('click', toggleSidebar);
    applyInitialSidebarState();
  }

  // 自動依視窗大小變化調整收合狀態
  window.addEventListener('resize', () => {
    if (appContainer && window.innerWidth <= 992) {
      setSidebarCollapsed(true);
    }
  });

  // 行動端側選單遮罩與關閉按鈕事件監聽
  const sidebarOverlay = document.getElementById('sidebar-overlay');
  const sidebarCloseBtn = document.getElementById('sidebar-close-btn');

  if (sidebarOverlay && appContainer) {
    sidebarOverlay.addEventListener('click', () => {
      setSidebarCollapsed(true, { persist: true });
    });
  }

  if (sidebarCloseBtn && appContainer) {
    sidebarCloseBtn.addEventListener('click', () => {
      setSidebarCollapsed(true, { persist: true });
    });
  }

  // 初始化深淺色主題切換
  initThemeToggle();

  // 初始化表格欄位排序
  initTableSorting();

  // 初始化前置設定教學 Modal 與事件
  initSetupGuide();

  // 載入費用標準規則
  fetchPricingRules();
  // 初始化費用標準 Modal 與事件
  initPricingModal();

  // 恢復即時自動刷新狀態 (無提示)
  const savedLiveRefreshOnLoad = getCookie('live_refresh') === 'true';
  if (savedLiveRefreshOnLoad && activeTab === 'daily') {
    toggleLiveRefresh(true, false);
  }
}

// =========================================================================
// Tab 切換邏輯
// =========================================================================
function switchTab(tab) {
  if (activeTab === tab) return;
  activeTab = tab;
  setCookie('active_tab', tab);
  updateUrlParams();

  // Update custom hover dropdown UI
  const tabLabelEls = document.querySelectorAll('.tab-dropdown-label');
  const tabHoverOpts = document.querySelectorAll('.tab-hover-dropdown .hover-dropdown-option');
  if (tabHoverOpts.length > 0) {
    tabHoverOpts.forEach(opt => {
      if (opt.getAttribute('data-value') === tab) {
        opt.classList.add('active');
        if (tabLabelEls.length > 0) {
          tabLabelEls.forEach(labelEl => {
            labelEl.setAttribute('data-i18n', opt.getAttribute('data-i18n'));
          });
        }
      } else {
        opt.classList.remove('active');
      }
    });
    if (typeof updateLanguageUI === 'function') {
      updateLanguageUI();
    }
  }

  const tabBtnDaily = document.getElementById('tab-btn-daily');
  const tabBtnMonthly = document.getElementById('tab-btn-monthly');
  const tabBtnYearly = document.getElementById('tab-btn-yearly');
  const dailySelector = document.getElementById('daily-selector-section');
  const monthlySelector = document.getElementById('monthly-selector-section');
  const yearlySelector = document.getElementById('yearly-selector-section');
  const quickStats = document.getElementById('quick-stats-section');
  const dailyView = document.getElementById('daily-view-container');
  const monthlyView = document.getElementById('monthly-view-container');
  const yearlyView = document.getElementById('yearly-view-container');

  const updateVisibility = (activeView) => {
    dailyView.classList.add('hidden');
    monthlyView.classList.add('hidden');
    yearlyView.classList.add('hidden');
    if (!isEmptyState && activeView) {
      activeView.classList.remove('hidden');
    }
  };

  if (tab === 'daily') {
    tabBtnDaily.classList.add('active');
    tabBtnMonthly.classList.remove('active');
    if (tabBtnYearly) tabBtnYearly.classList.remove('active');
    dailySelector.classList.remove('hidden');
    monthlySelector.classList.add('hidden');
    if (yearlySelector) yearlySelector.classList.add('hidden');
    quickStats.classList.remove('hidden');
    
    updateVisibility(dailyView);

    // 載入當前日期的數據
    const dateSelect = document.getElementById('date-select');
    if (dateSelect.value) {
      loadUsageData(dateSelect.value);
    }
  } else if (tab === 'monthly') {
    // 關閉即時自動刷新以節省資源
    const liveToggle = document.getElementById('live-toggle');
    if (liveToggle && liveToggle.checked) {
      liveToggle.checked = false;
      toggleLiveRefresh(false);
      setCookie('live_refresh', 'false');
    }

    tabBtnDaily.classList.remove('active');
    tabBtnMonthly.classList.add('active');
    if (tabBtnYearly) tabBtnYearly.classList.remove('active');
    dailySelector.classList.add('hidden');
    monthlySelector.classList.remove('hidden');
    if (yearlySelector) yearlySelector.classList.add('hidden');
    quickStats.classList.add('hidden');
    
    updateVisibility(monthlyView);

    // 載入當前月份的數據
    const monthSelect = document.getElementById('month-select');
    if (monthSelect.value) {
      loadMonthlyData(monthSelect.value);
    } else {
      fetchMonths();
    }
  } else if (tab === 'yearly') {
    // 關閉即時自動刷新以節省資源
    const liveToggle = document.getElementById('live-toggle');
    if (liveToggle && liveToggle.checked) {
      liveToggle.checked = false;
      toggleLiveRefresh(false);
      setCookie('live_refresh', 'false');
    }

    tabBtnDaily.classList.remove('active');
    tabBtnMonthly.classList.remove('active');
    if (tabBtnYearly) tabBtnYearly.classList.add('active');
    dailySelector.classList.add('hidden');
    monthlySelector.classList.add('hidden');
    if (yearlySelector) yearlySelector.classList.remove('hidden');
    quickStats.classList.add('hidden');
    
    updateVisibility(yearlyView);

    // 載入當前年份的數據
    const yearSelect = document.getElementById('year-select');
    if (yearSelect && yearSelect.value) {
      loadYearlyData(yearSelect.value);
    } else {
      fetchYears();
    }
  }
  updateCodexRateLimit();
}

// =========================================================================
// 即時監控自動重新整理 (Live Auto-Refresh)
// =========================================================================
function toggleLiveRefresh(enabled, showToast = true) {
  const panel = document.getElementById('live-settings-panel');
  const dateSelect = document.getElementById('date-select');
  const btnToday = document.getElementById('btn-today');
  const btnPrevDay = document.getElementById('btn-prev-day');
  const btnNextDay = document.getElementById('btn-next-day');

  if (enabled) {
    panel.style.display = 'block';
    dateSelect.disabled = true; // 鎖定日期選擇
    if (btnToday) btnToday.disabled = true; // 鎖定今日按鈕
    if (btnPrevDay) btnPrevDay.disabled = true;
    if (btnNextDay) btnNextDay.disabled = true;

    // 自動切換到當天的日期 (以今天日期進行即時監控)
    const todayStr = getLocalDateString();
    dateSelect.value = todayStr;
    loadUsageData(todayStr);

    startLiveRefresh();
    if (showToast) {
      showNotification(t('live_refresh_enabled'), 'success');
    }
  } else {
    panel.style.display = 'none';
    dateSelect.disabled = false;
    if (btnToday) btnToday.disabled = false;
    if (btnPrevDay) btnPrevDay.disabled = false;
    if (btnNextDay) btnNextDay.disabled = false;

    stopLiveRefresh();
    if (showToast) {
      showNotification(t('live_refresh_disabled'), 'info');
    }
  }
}

function startLiveRefresh() {
  stopLiveRefresh();

  const intervalInput = document.getElementById('live-interval');
  refreshInterval = parseInt(intervalInput.value, 10);

  const statusText = document.getElementById('live-status-text');
  const progressBar = document.getElementById('refresh-progress');
  
  progressBar.style.width = '0%';

  let startTime = Date.now();
  
  // 100ms 進度條更新一次以確保極度順暢
  liveProgressTimer = setInterval(() => {
    let elapsed = Date.now() - startTime;
    let percentage = Math.min((elapsed / refreshInterval) * 100, 100);
    progressBar.style.width = `${percentage}%`;

    let seconds = Math.max(Math.ceil((refreshInterval - elapsed) / 1000), 0);
    statusText.textContent = t('status_monitoring').replace('{sec}', seconds);
  }, 100);

  // 實際刷新 API 的定時器
  liveRefreshTimer = setInterval(async () => {
    // 重設進度條與時間
    startTime = Date.now();
    progressBar.style.width = '0%';

    // 重新載入最新資料
    await refreshLiveData();
  }, refreshInterval);
}

function stopLiveRefresh() {
  if (liveRefreshTimer) {
    clearInterval(liveRefreshTimer);
    liveRefreshTimer = null;
  }
  if (liveProgressTimer) {
    clearInterval(liveProgressTimer);
    liveProgressTimer = null;
  }
  const progressBar = document.getElementById('refresh-progress');
  if (progressBar) progressBar.style.width = '0%';
}

async function refreshLiveData() {
  try {
    const res = await fetch(`/api/${currentAssistant}/dates`);
    const data = await res.json();
    availableDates = data.dates || [];
    
    const dateSelect = document.getElementById('date-select');
    const todayStr = getLocalDateString();
    
    // 更新日曆的最小與最大限制
    if (availableDates.length > 0) {
      dateSelect.min = availableDates[availableDates.length - 1];
    }
    dateSelect.max = todayStr;

    // 即時自動刷新跨日支援：若目前時間已進入新的一天且與當前選擇不同，自動切換
    if (dateSelect.value !== todayStr) {
      console.log(`即時監控跨日切換: ${dateSelect.value} -> ${todayStr}`);
      dateSelect.value = todayStr;
      showNotification(`${t('detected_new_day') || '已跨日，自動切換至新的一天：'}${todayStr}`, 'info');
    }

    // 載入所選日期 (即新的 todayStr) 數據
    await loadUsageData(dateSelect.value);
  } catch (err) {
    console.error('即時刷新失敗:', err);
    const statusText = document.getElementById('live-status-text');
    if (statusText) statusText.textContent = t('status_failed');
  }
}

// =========================================================================
// API 呼叫: 載入日期清單
// =========================================================================
async function fetchDates(selectedDate = null, keepDate = false) {
  try {
    const res = await fetch(`/api/${currentAssistant}/dates`);
    const data = await res.json();
    
    const dateSelect = document.getElementById('date-select');
    availableDates = data.dates || [];

    if (availableDates.length === 0 && !keepDate) {
      toggleEmptyState(true);
      return;
    }

    // 設定日曆最小與最大值
    const oldestDate = availableDates.length > 0 ? availableDates[availableDates.length - 1] : null;
    const newestDate = availableDates.length > 0 ? availableDates[0] : null;
    const todayStr = getLocalDateString();
    
    if (oldestDate) dateSelect.min = oldestDate;
    dateSelect.max = todayStr;

    let dateToLoad;
    if (keepDate) {
      // 切換 agent：保留目前日期，不自動跳轉
      dateToLoad = dateSelect.value || todayStr;
    } else {
      const urlDate = getUrlDateForTab('daily');
      dateToLoad = selectedDate || urlDate || dateSelect.value;
      if (!dateToLoad || (!selectedDate && !urlDate && !availableDates.includes(dateToLoad))) {
        // 若有啟用即時刷新，預設為今日；否則預設為最新有日誌的日期
        const liveToggle = document.getElementById('live-toggle');
        if (liveToggle && liveToggle.checked) {
          dateToLoad = todayStr;
        } else {
          dateToLoad = newestDate || todayStr;
        }
      }
      dateSelect.value = dateToLoad;
      toggleEmptyState(false);
    }

    // 載入所選日期的數據（keepDate 時即使不在清單也直接請求，讓後端回 404）
    // 若目前在 monthly tab，不呼叫 loadUsageData（避免 showNoDataForDate 蓋掉月報畫面）
    if (!keepDate || activeTab === 'daily') {
      await loadUsageData(dateToLoad);
    }

  } catch (err) {
    console.error('獲取日期清單失敗:', err);
    showNotification(t('server_conn_failed'), 'error');
  }
}

async function reloadDailyData() {
  const dateSelect = document.getElementById('date-select');
  const selectedDate = dateSelect.value;
  await fetchDates(selectedDate);
}

// =========================================================================
// API 呼叫: 載入當日使用量數據
// =========================================================================
async function loadUsageData(date) {
  if (!date || date === 'undefined' || date === 'null') {
    return;
  }
  updateUrlParams();
  try {
    // 顯示加載動畫 (可在此擴展)
    setTitleMarkup('sync', date);

    const res = await fetch(`/api/${currentAssistant}/usage/${date}`);
    if (res.status === 404) {
      // 顯示「此 Agent 當日無資料」提示畫面，不改變日期
      showNoDataForDate(date);
      await updateCodexRateLimit();
      return;
    }
    
    const data = await res.json();
    toggleEmptyState(false);
    renderDashboard(data);
    await updateCodexRateLimit();

  } catch (err) {
    console.error('載入使用量失敗:', err);
    showNotification(t('load_failed'), 'error');
  }
}

// 顯示「此 Agent 於當日無資料」的提示畫面
function showNoDataForDate(date) {
  const meta = getAssistantMeta(currentAssistant);
  const title = t('no_data_for_date')
    .replace('{agent}', meta.label)
    .replace('{date}', date);
  const desc = t('no_data_for_date_desc');
  const logoMarkup = `<div class="card-icon"><img src="${meta.logo}" alt="${meta.alt}" style="width: 56px; height: 56px; object-fit: contain;" /></div>`;

  const emptyContainer = document.getElementById('empty-state-container');
  const dailyView = document.getElementById('daily-view-container');
  const monthlyView = document.getElementById('monthly-view-container');
  const yearlyView = document.getElementById('yearly-view-container');

  if (emptyContainer) {
    emptyContainer.classList.remove('hidden');
    emptyContainer.innerHTML = `
      <div class="welcome-setup-card no-agent-card" style="align-items: center; text-align: center;">
        ${logoMarkup}
        <h2>${title}</h2>
        <p style="text-align: center; max-width: 100%;">${desc}</p>
        <div class="action-buttons">
          <button class="primary-btn" id="btn-no-data-setup-guide">${t('btn_empty_setup')}</button>
          <button class="secondary-btn" id="btn-no-data-refresh">${t('btn_empty_refresh')}</button>
        </div>
      </div>
    `;

    const noDataGuideBtn = document.getElementById('btn-no-data-setup-guide');
    if (noDataGuideBtn) {
      noDataGuideBtn.addEventListener('click', openSetupModal);
    }

    const noDataRefreshBtn = document.getElementById('btn-no-data-refresh');
    if (noDataRefreshBtn) {
      noDataRefreshBtn.addEventListener('click', async () => {
        noDataRefreshBtn.classList.add('loading');
        try {
          const dateSelect = document.getElementById('date-select');
          if (dateSelect) {
            dateSelect.value = date;
          }
          await fetchDates(null, true);
        } finally {
          noDataRefreshBtn.classList.remove('loading');
        }
      });
    }
  }
  if (dailyView) dailyView.classList.add('hidden');
  if (monthlyView) monthlyView.classList.add('hidden');
  if (yearlyView) yearlyView.classList.add('hidden');

  // 更新標題
  setTitleMarkup('empty', date);
}

// Helpers to render metrics values (handling agent breakdown when multiple agents are active)
function getActiveAgents() {
  const activeAgents = [];
  document.querySelectorAll('.assistant-badge-btn.active').forEach(b => {
    activeAgents.push(normalizeAssistant(b.getAttribute('data-value')));
  });
  return activeAgents;
}

function renderMetricValue(elementId, getValFn, formatFn, sessions, activeAgents) {
  const el = document.getElementById(elementId);
  if (!el) return;
  
  const isMulti = activeAgents.length > 1;
  if (!isMulti) {
    const totalVal = sessions.reduce((sum, s) => sum + getValFn(s), 0);
    el.innerHTML = formatFn(totalVal);
  } else {
    const agentData = {};
    activeAgents.forEach(a => {
      agentData[a] = 0;
    });
    sessions.forEach(s => {
      if (agentData[s.assistant_type] !== undefined) {
        agentData[s.assistant_type] += getValFn(s);
      }
    });
    
    let html = '<div class="stat-value-list">';
    activeAgents.forEach(a => {
      const meta = getAssistantMeta(a);
      let logoUrl = meta.logo;
      let displayName = meta.label;
      html += `
        <div class="stat-value-item">
          <span class="agent-name" title="${displayName}"><img class="badge-logo" src="${logoUrl}" alt="${displayName}" /></span>
          <span class="val">${formatFn(agentData[a])}</span>
        </div>
      `;
    });
    html += '</div>';
    el.innerHTML = html;
  }
}

function renderMonthlyMetricValue(elementId, getValFn, formatFn, agentBreakdown, activeAgents) {
  const el = document.getElementById(elementId);
  if (!el) return;
  
  const isMulti = activeAgents.length > 1;
  if (!isMulti) {
    // Falls back to showing single total summary value
    // Rendered directly in renderMonthlyDashboard
  } else {
    let html = '<div class="stat-value-list">';
    activeAgents.forEach(a => {
      const meta = getAssistantMeta(a);
      let logoUrl = meta.logo;
      let displayName = meta.label;
      
      const val = (agentBreakdown && agentBreakdown[a]) ? getValFn(agentBreakdown[a]) : 0;
      html += `
        <div class="stat-value-item">
          <span class="agent-name" title="${displayName}"><img class="badge-logo" src="${logoUrl}" alt="${displayName}" /></span>
          <span class="val">${formatFn(val)}</span>
        </div>
      `;
    });
    html += '</div>';
    el.innerHTML = html;
  }
}

// =========================================================================
// 渲染主看板數據
// =========================================================================
function renderDashboard(data) {
  currentUsageData = data;
  const { date, summary, sessions } = data;

  // 1. 更新標題
  setTitleMarkup('calendar', date);

  // 2. 更新側邊欄指標卡片
  document.getElementById('mini-sessions').textContent = summary.total_sessions;
  document.getElementById('mini-tokens').textContent = formatToken(summary.total_tokens);
  document.getElementById('mini-cache').textContent = `${t('cache_read_label')}: ${formatToken(summary.total_cache_read_tokens)}`;
  document.getElementById('mini-cost').textContent = formatCost(summary.total_cost_usd || 0);
  document.getElementById('mini-duration').textContent = formatDuration(summary.total_duration_ms);
  document.getElementById('mini-requests').textContent = summary.total_requests;

  // 3. 更新主看板 Metric Cards
  const activeAgents = getActiveAgents();
  const isMulti = activeAgents.length > 1;
  const rawInputTokens = summary.total_input_tokens || 0;
  const cacheReadTokens = summary.total_cache_read_tokens || 0;
  const reasoningTokens = summary.total_reasoning_tokens || 0;
  const netInputTokens = getNetInputTokens(rawInputTokens, cacheReadTokens);
  const combinedInputTokens = netInputTokens + reasoningTokens;

  if (!isMulti) {
    document.getElementById('stat-total-tokens').textContent = formatToken(summary.total_tokens);
    document.getElementById('stat-input-tokens').textContent = formatToken(combinedInputTokens);
    document.getElementById('stat-cache-read-tokens').textContent = formatToken(cacheReadTokens);
    document.getElementById('stat-output-tokens').textContent = formatToken(summary.total_output_tokens);
    document.getElementById('stat-total-cost').textContent = formatCost(summary.total_cost_usd || 0);
  } else {
    renderMetricValue('stat-total-tokens', s => s.total_tokens, formatToken, sessions, activeAgents);
    renderMetricValue('stat-input-tokens', getDisplayedInputTokens, formatToken, sessions, activeAgents);
    renderMetricValue('stat-cache-read-tokens', s => s.total_cache_read_tokens || 0, formatToken, sessions, activeAgents);
    renderMetricValue('stat-output-tokens', s => s.total_output_tokens, formatToken, sessions, activeAgents);
    renderMetricValue('stat-total-cost', s => s.cost_usd || 0, formatCost, sessions, activeAgents);
  }

  const statInputReasoning = document.getElementById('stat-input-reasoning');
  const statInputLabel = document.getElementById('stat-input-label');
  const statInputTokens = document.getElementById('stat-input-tokens');
  const statCacheReadLabel = document.getElementById('stat-cache-read-label');
  const statCacheWrite = document.getElementById('stat-cache-write');
  const statOutputLabel = document.getElementById('stat-output-label');
  const inputPercent = calculatePercentage(combinedInputTokens, summary.total_tokens);
  const cacheReadPercent = calculatePercentage(cacheReadTokens, summary.total_tokens);
  const outputPercent = calculatePercentage(summary.total_output_tokens || 0, summary.total_tokens);
  const reasoningPercent = calculatePercentage(reasoningTokens, summary.total_tokens);

  if (statInputLabel) {
    const inputTooltip = t('input_tokens_percentage_formula')
      .replace('{input}', formatNumber(rawInputTokens))
      .replace('{cacheRead}', formatNumber(cacheReadTokens))
      .replace('{netInput}', formatNumber(netInputTokens))
      .replace('{reasoning}', formatNumber(reasoningTokens))
      .replace('{combined}', formatNumber(combinedInputTokens))
      .replace('{total}', formatNumber(summary.total_tokens || 0))
      .replace('{percent}', inputPercent);
    statInputLabel.textContent = `${t('input_tokens_label')} (${inputPercent})`;
    statInputLabel.title = inputTooltip;
    statInputLabel.setAttribute('aria-label', inputTooltip);
  }
  if (statInputTokens) {
    const inputValueTooltip = t('reasoning_tokens_value_tooltip')
      .replace('{netInput}', formatToken(netInputTokens))
      .replace('{reasoning}', formatToken(reasoningTokens))
      .replace('{percent}', reasoningPercent);
    statInputTokens.title = inputValueTooltip;
    statInputTokens.setAttribute('aria-label', `${formatToken(combinedInputTokens)}; ${inputValueTooltip}`);
  }
  if (statCacheReadLabel) {
    const cacheReadTooltip = t('cache_read_percentage_formula')
      .replace('{cacheRead}', formatNumber(cacheReadTokens))
      .replace('{total}', formatNumber(summary.total_tokens || 0))
      .replace('{percent}', cacheReadPercent);
    statCacheReadLabel.textContent = `${t('chart_cache_label')} (${cacheReadPercent})`;
    statCacheReadLabel.title = cacheReadTooltip;
    statCacheReadLabel.setAttribute('aria-label', cacheReadTooltip);
  }
  if (statOutputLabel) {
    const outputTooltip = t('output_tokens_percentage_formula')
      .replace('{output}', formatNumber(summary.total_output_tokens || 0))
      .replace('{total}', formatNumber(summary.total_tokens || 0))
      .replace('{percent}', outputPercent);
    statOutputLabel.textContent = `${t('output_tokens_label')} (${outputPercent})`;
    statOutputLabel.title = outputTooltip;
    statOutputLabel.setAttribute('aria-label', outputTooltip);
  }

  if (isMulti) {
    if (statInputReasoning) statInputReasoning.classList.add('hidden');
    if (statCacheWrite) statCacheWrite.classList.add('hidden');
  } else {
    if (statInputReasoning) {
      statInputReasoning.classList.add('hidden');
      statInputReasoning.textContent = '';
    }
    if (statCacheWrite) {
      const cacheWriteTokens = summary.total_cache_write_tokens || 0;
      if (cacheWriteTokens > 0) {
        statCacheWrite.classList.remove('hidden');
        statCacheWrite.textContent = `${t('cache_write_label')}: ${formatToken(cacheWriteTokens)}`;
      } else {
        statCacheWrite.classList.add('hidden');
        statCacheWrite.textContent = '';
      }
    }
  }

  // 4. 繪製 Token 圖表
  renderChart(sessions);

  // 5. 渲染 Session 列表
  currentSessions = [...sessions];
  sortAndRenderSessionTable();
}

// =========================================================================
// 渲染 Chart.js Token 使用趨勢圖
// =========================================================================
function renderChart(sessions) {
  const canvas = document.getElementById('tokenChart');

  // 只取前 15 個 Session 來畫，避免過於擁擠
  const sortedSessions = [...sessions].sort((a, b) => a.timestamp.localeCompare(b.timestamp));
  const displaySessions = sortedSessions.slice(-15);

  currentChartSessions = displaySessions;

  const labels = displaySessions.map((s, idx) => {
    const timeStr = s.timestamp ? formatLocalTime(s.timestamp, false) : '';
    return `${timeStr} (${s.session_name.substring(0, 10)}...)`;
  });

  const tokenData = displaySessions.map(s => s.total_tokens);
  const cacheData = displaySessions.map(s => s.total_cache_read_tokens || 0);
  const maxTurnData = displaySessions.map(s => s.max_turn_no);

  // 若圖表已存在，則動態更新數據以達到平滑變動效果
  if (tokenChartInstance) {
    tokenChartInstance.data.labels = labels;
    tokenChartInstance.data.datasets[0].label = t('chart_token_label');
    tokenChartInstance.data.datasets[1].label = t('chart_cache_label');
    tokenChartInstance.data.datasets[2].label = t('chart_turn_label');
    tokenChartInstance.data.datasets[0].data = tokenData;
    tokenChartInstance.data.datasets[1].data = cacheData;
    tokenChartInstance.data.datasets[2].data = maxTurnData;
    if (tokenChartInstance.options.scales && tokenChartInstance.options.scales.y && tokenChartInstance.options.scales.y.title) {
      tokenChartInstance.options.scales.y.title.text = t('col_total');
    }
    if (tokenChartInstance.options.scales && tokenChartInstance.options.scales.y1 && tokenChartInstance.options.scales.y1.title) {
      tokenChartInstance.options.scales.y1.title.text = t('col_turns');
    }
    tokenChartInstance.update();
    return;
  }

  tokenChartInstance = new Chart(canvas, {
    type: 'bar',
    data: {
      labels: labels,
      datasets: [
        {
          label: t('chart_token_label'),
          data: tokenData,
          backgroundColor: chartPalette.tokenFill,
          borderColor: chartPalette.tokenStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_cache_label'),
          data: cacheData,
          backgroundColor: chartPalette.cacheFill,
          borderColor: chartPalette.cacheStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_turn_label'),
          data: maxTurnData,
          type: 'line',
          borderColor: chartPalette.trendStroke,
          backgroundColor: chartPalette.trendFill,
          borderWidth: 2,
          pointBackgroundColor: chartPalette.trendStroke,
          pointRadius: 4,
          tension: 0.3,
          yAxisID: 'y1',
        }
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      onClick: (event, elements) => {
        if (elements && elements.length > 0) {
          const index = elements[0].index;
          const session = currentChartSessions[index];
          if (session) {
            openSessionTimeline(session.session_id, session.session_name, session.total_tokens, session.total_cache_read_tokens);
          }
        }
      },
      onHover: (event, activeElements) => {
        canvas.style.cursor = activeElements.length ? 'pointer' : 'default';
      },
      plugins: {
        legend: {
          labels: {
            color: '#f3f4f6',
            font: {
              family: chartFontFamily
            }
          }
        },
        tooltip: {
          padding: 12,
          backgroundColor: 'rgba(15, 18, 29, 0.95)',
          titleColor: chartPalette.tokenStroke,
          bodyColor: '#f3f4f6',
          borderColor: 'rgba(255, 255, 255, 0.1)',
          borderWidth: 1,
          callbacks: {
            label: (context) => {
              const label = context.dataset.label || '';
              const value = context.parsed.y;
              if (label.includes('Token')) {
                return `${label}: ${formatToken(value)} (${formatNumber(value)})`;
              }
              return `${label}: ${formatNumber(value)}`;
            }
          }
        }
      },
      scales: {
        x: {
          stacked: false,
          grid: {
            color: 'rgba(255, 255, 255, 0.05)'
          },
          ticks: {
            color: '#9ca3af',
            font: {
              size: 10
            }
          }
        },
        y: {
          stacked: false,
          type: 'linear',
          position: 'left',
          grid: {
            color: 'rgba(255, 255, 255, 0.05)'
          },
          ticks: {
            color: '#9ca3af',
            callback: (value) => formatToken(value)
          },
          title: {
            display: true,
            text: t('col_total'),
            color: '#f3f4f6'
          }
        },
        y1: {
          stacked: false,
          type: 'linear',
          position: 'right',
          grid: {
            drawOnChartArea: false, // 不畫右邊 y1 的格線避免混淆
          },
          ticks: {
            color: '#9ca3af',
            stepSize: 1
          },
          title: {
            display: true,
            text: t('col_turns')
          }
        }
      }
    }
  });

  // 根據當前主題更新圖表樣式
  const currentTheme = document.documentElement.getAttribute('data-theme') || 'dark';
  updateChartsTheme(currentTheme);
}

// =========================================================================
// 會話列表排序邏輯與事件監聽
// =========================================================================
function initTableSorting() {
  const headers = document.querySelectorAll('.premium-table th.sortable');
  headers.forEach(th => {
    th.addEventListener('click', () => {
      const column = th.getAttribute('data-sort');
      const tableType = th.getAttribute('data-table');
      
      if (tableType === 'yearly') {
        // 年度每月彙總表格排序
        if (yearlyMonthlySortColumn === column) {
          yearlyMonthlySortDirection = yearlyMonthlySortDirection === 'asc' ? 'desc' : 'asc';
        } else {
          yearlyMonthlySortColumn = column;
          yearlyMonthlySortDirection = 'desc'; // 預設降冪排序
        }
        sortAndRenderYearlyMonthlyTable();
      } else if (tableType === 'monthly') {
        // 月度每日彙總表格排序
        if (monthlyDailySortColumn === column) {
          monthlyDailySortDirection = monthlyDailySortDirection === 'asc' ? 'desc' : 'asc';
        } else {
          monthlyDailySortColumn = column;
          monthlyDailySortDirection = 'desc'; // 預設降冪排序
        }
        sortAndRenderMonthlyDailyTable();
      } else {
        // 會話列表排序
        if (currentSortColumn === column) {
          // 切換排序方向
          currentSortDirection = currentSortDirection === 'asc' ? 'desc' : 'asc';
        } else {
          currentSortColumn = column;
          // 數值欄位預設由大到小排序，字串/時間欄位預設由小到大排序
          const numericColumns = [
            'max_turn_no', 
            'total_input_tokens', 
            'total_output_tokens', 
            'total_cache_read_tokens', 
            'total_tokens', 
            'duration_ms'
          ];
          currentSortDirection = numericColumns.includes(column) ? 'desc' : 'asc';
        }
        sortAndRenderSessionTable();
      }
    });
  });
}

function sortAndGetFlatSessions(sessions, sortCol, sortDir) {
  const map = {};
  sessions.forEach(s => {
    map[s.session_id] = { ...s, children: [] };
  });

  const roots = [];
  sessions.forEach(s => {
    const item = map[s.session_id];
    if (s.parent_session_id && map[s.parent_session_id]) {
      map[s.parent_session_id].children.push(item);
    } else {
      roots.push(item);
    }
  });

  const compare = (a, b) => {
    let valA = a[sortCol];
    let valB = b[sortCol];
    if (valA === undefined || valA === null) valA = 0;
    if (valB === undefined || valB === null) valB = 0;

    if (typeof valA === 'string' && typeof valB === 'string') {
      return sortDir === 'asc' ? valA.localeCompare(valB) : valB.localeCompare(valA);
    }
    return sortDir === 'asc' ? valA - valB : valB - valA;
  };

  // 排序 Root 節點
  roots.sort(compare);

  // 遞迴排序子節點
  const sortTree = (node) => {
    node.children.sort(compare);
    node.children.forEach(sortTree);
  };
  roots.forEach(sortTree);

  // 扁平化
  const flat = [];
  const traverse = (node, depth, parentName) => {
    flat.push({
      ...node,
      depth,
      isSubagent: depth > 0,
      parentName
    });
    node.children.forEach(child => traverse(child, depth + 1, node.session_name));
  };
  roots.forEach(r => traverse(r, 0, null));

  return flat;
}

function sortAndRenderSessionTable() {
  if (!currentSessions || currentSessions.length === 0) {
    renderSessionTable([]);
    return;
  }

  const flatSessions = sortAndGetFlatSessions(currentSessions, currentSortColumn, currentSortDirection);
  renderSessionTable(flatSessions);
  updateSortHeadersUI();
}

function updateSortHeadersUI() {
  const headers = document.querySelectorAll('.premium-table th.sortable:not([data-table="monthly"])');
  headers.forEach(th => {
    const column = th.getAttribute('data-sort');
    const icon = th.querySelector('.sort-icon');
    if (!icon) return;

    th.classList.remove('sorted-asc', 'sorted-desc');
    
    if (column === currentSortColumn) {
      if (currentSortDirection === 'asc') {
        th.classList.add('sorted-asc');
        icon.innerHTML = iconMarkup('chevron-up', 'sort-glyph');
      } else {
        th.classList.add('sorted-desc');
        icon.innerHTML = iconMarkup('chevron-down', 'sort-glyph');
      }
    } else {
      icon.innerHTML = `<span class="sort-icon-placeholder">${iconMarkup('chevron-up', 'sort-glyph')}${iconMarkup('chevron-down', 'sort-glyph')}</span>`;
    }
  });
}

// =========================================================================
// 渲染 Session 列表 Table
// =========================================================================
function renderSessionTable(sessions) {
  const tbody = document.getElementById('session-list-body');
  document.getElementById('session-count').textContent = `${sessions.length} Sessions`;
  tbody.innerHTML = '';

  const colHeader = document.getElementById('col-assistant-header');
  if (colHeader) {
    if (currentAssistant === 'all' || currentAssistant.includes(',')) {
      colHeader.classList.remove('hidden');
    } else {
      colHeader.classList.add('hidden');
    }
  }

  if (sessions.length === 0) {
    tbody.innerHTML = `<tr><td colspan="12" class="placeholder-text">${t('placeholder_no_sessions')}</td></tr>`;
    return;
  }

  // 建立快速查詢 Map 以供 Hover 高亮與樹狀結構查詢
  const sessionsMap = {};
  sessions.forEach(s => {
    sessionsMap[s.session_id] = s;
  });

  function getRootParentId(session) {
    let curr = session;
    while (curr && curr.parent_session_id && sessionsMap[curr.parent_session_id]) {
      curr = sessionsMap[curr.parent_session_id];
    }
    return curr ? curr.session_id : session.session_id;
  }

  sessions.forEach(s => {
    const tr = document.createElement('tr');
    tr.setAttribute('data-session-id', s.session_id);
    tr.setAttribute('data-parent-id', s.parent_session_id || '');

    if (s.isSubagent) {
      tr.classList.add('subagent-row');
    }
    
    // 格式化時間
    const timeFormatted = s.timestamp ? formatLocalTime(s.timestamp, true) : '-';

    let assistantBadge = "";
    if (isSupportedAssistant(s.assistant_type)) {
      const meta = getAssistantMeta(s.assistant_type);
      assistantBadge = `<span class="badge" style="${meta.badgeStyle}">${getAssistantLogoHtml(s.assistant_type)} ${meta.shortLabel}</span>`;
    }

    const astColumn = (currentAssistant === 'all' || currentAssistant.includes(',')) ? `<td>${assistantBadge}</td>` : '';

    // 依據 depth 縮排會話名稱，並呈現└─ 符號與 subagent tag
    let nameCellContent = '';
    if (s.isSubagent) {
      const paddingLeft = s.depth * 16;
      const connectorLeft = (s.depth - 1) * 16 + 4;
      const nickname = s.agent_nickname || '';
      const role = s.agent_role || '';
      nameCellContent = `
        <div class="session-name-wrapper is-subagent" style="padding-left: ${paddingLeft}px;">
          <span class="tree-connector" style="left: ${connectorLeft}px;">└─</span>
          <div style="display: flex; flex-wrap: wrap; gap: 4px; align-items: center; margin-bottom: 3px;">
            <span class="badge subagent-badge" title="Subagent of: ${escapeHtml(s.parentName || '')}">Subagent</span>
            ${nickname ? `<span class="badge agent-nickname-badge" title="Agent Nickname: ${escapeHtml(nickname)}">${escapeHtml(nickname)}</span>` : ''}
            ${role ? `<span class="badge agent-role-badge" title="Agent Role: ${escapeHtml(role)}">${escapeHtml(role)}</span>` : ''}
          </div>
          <span class="session-name-text" title="${escapeHtml(s.session_name)}">${escapeHtml(s.session_name)}</span>
          <span class="session-id-sub">${s.session_id}</span>
        </div>
      `;
    } else {
      nameCellContent = `
        <div class="session-name-wrapper">
          <span class="session-name-text" title="${escapeHtml(s.session_name)}">${escapeHtml(s.session_name)}</span>
          <span class="session-id-sub">${s.session_id}</span>
        </div>
      `;
    }

    tr.innerHTML = `
      <td class="session-name-cell">
        ${nameCellContent}
      </td>
      ${astColumn}
      <td class="model-column">
        <div class="model-cell-content">
          <span class="badge highlight">${escapeHtml(s.model)}</span>
          ${s.reasoning_effort ? `<span class="badge" style="background: rgba(127, 142, 163, 0.15); color: #aeb9c8; font-size: 11px; font-weight: 600;">${escapeHtml(s.reasoning_effort)}</span>` : ''}
        </div>
      </td>
      <td><span class="badge">${s.max_turn_no}</span></td>
      <td style="color: var(--text-secondary);">${formatToken(s.total_input_tokens || 0)}</td>
      <td style="color: var(--text-secondary);">${formatToken(s.total_output_tokens || 0)}</td>
      <td style="color: #aeb9c8;">${formatToken(s.total_reasoning_tokens || 0)}</td>
      <td style="color: #34d399;">${formatToken(s.total_cache_read_tokens || 0)}</td>
      <td style="font-weight: 700; color: #fbbf24;">${formatToken(s.total_tokens)}</td>
      <td style="font-weight: 700; color: var(--accent-cyan);">${formatCost(s.cost_usd || 0)}</td>
      <td>${formatDuration(s.duration_ms)}</td>
      <td style="color: var(--text-secondary);">${timeFormatted}</td>
    `;

    // 當點擊 Session 時，開啟對話詳細還原
    tr.addEventListener('click', () => {
      openSessionTimeline(
        s.session_id,
        s.session_name,
        s.total_tokens,
        s.total_cache_read_tokens,
        s.total_input_tokens,
        s.total_output_tokens,
        s.total_reasoning_tokens,
        s.cwd,
        s.model,
        s.assistant_type,
        s.agent_nickname,
        s.agent_role,
        s.reasoning_effort
      );
    });

    // 群組 Hover 高亮
    tr.addEventListener('mouseenter', () => {
      const rootId = getRootParentId(s);
      tbody.querySelectorAll('tr').forEach(row => {
        const sid = row.getAttribute('data-session-id');
        const pid = row.getAttribute('data-parent-id');
        const rowSession = sessionsMap[sid];
        
        if (sid === rootId || pid === rootId || (rowSession && getRootParentId(rowSession) === rootId)) {
          row.classList.add('family-highlight');
        }
      });
    });

    tr.addEventListener('mouseleave', () => {
      tbody.querySelectorAll('tr').forEach(row => {
        row.classList.remove('family-highlight');
      });
    });

    tbody.appendChild(tr);
  });
}

// =========================================================================
// API 呼叫: 載入並渲染特定 Session 對話時間軸 (Timeline)
// =========================================================================
async function openSessionTimeline(sessionId, sessionName, totalTokens, cacheReadTokens, inputTokens, outputTokens, reasoningTokens, cwd, model, assistantType, agentNickname, agentRole) {
  const drawerOverlay = document.getElementById('timeline-drawer');
  const timelineContainer = document.getElementById('timeline-items');

  // 保存當前點擊之 Session 的正確統計與資訊以作為 Fallback
  currentSessionTotalTokens = totalTokens || 0;
  currentSessionCacheTokens = cacheReadTokens || 0;
  currentSessionInputTokens = inputTokens || 0;
  currentSessionOutputTokens = outputTokens || 0;
  currentSessionReasoningTokens = reasoningTokens || 0;
  currentSessionCwd = cwd || '';
  currentSessionModel = model || '';
  currentSessionAssistantType = assistantType || '';

  // 設定基礎抬頭 (截斷至 100 字元，滑鼠移過去可以看到全部)
  let displayName = sessionName || '';
  if (displayName.length > 100) {
    displayName = displayName.substring(0, 100) + '...';
  }
  const nameEl = document.getElementById('drawer-session-name');
  nameEl.textContent = displayName;
  nameEl.title = sessionName || '';
  document.getElementById('drawer-session-id').textContent = sessionId;

  // 更新會話 Token & 基礎資訊（立即呈現在畫面上）
  document.getElementById('meta-cwd').textContent = cwd || '-';
  document.getElementById('meta-cwd').title = cwd || '';
  document.getElementById('meta-model').textContent = model || '-';
  const metaEffort = document.getElementById('meta-effort');
  if (metaEffort) {
    metaEffort.textContent = '-';
    metaEffort.style.display = 'none';
  }
  document.getElementById('meta-tokens').textContent = formatToken(totalTokens || 0);
  document.getElementById('meta-cache').textContent = formatToken(cacheReadTokens || 0);
  document.getElementById('meta-compaction').textContent = '-';
  document.getElementById('meta-input').textContent = formatToken(inputTokens || 0);
  document.getElementById('meta-output').textContent = formatToken(outputTokens || 0);
  document.getElementById('meta-reasoning').textContent = formatToken(reasoningTokens || 0);

  const nicknameContainer = document.getElementById('drawer-meta-nickname-container');
  const roleContainer = document.getElementById('drawer-meta-role-container');

  if (agentNickname) {
    document.getElementById('meta-nickname').textContent = agentNickname;
    if (nicknameContainer) nicknameContainer.style.display = 'flex';
  } else {
    if (nicknameContainer) nicknameContainer.style.display = 'none';
  }

  if (agentRole) {
    document.getElementById('meta-role').textContent = agentRole;
    if (roleContainer) roleContainer.style.display = 'flex';
  } else {
    if (roleContainer) roleContainer.style.display = 'none';
  }

  // 顯示加載動畫
  timelineContainer.innerHTML = `<div class="placeholder-text">${t('drawer_loading')}</div>`;
  
  // 顯示抽屜面板
  drawerOverlay.classList.add('active');

  try {
    const resolvedAssistant = assistantType || currentAssistant;
    const res = await fetch(`/api/${encodeURIComponent(resolvedAssistant)}/session/${encodeURIComponent(sessionId)}`);
    if (res.status === 404) {
      const errData = await res.json().catch(() => ({}));
      if (errData.reason === 'no_events_yet') {
        timelineContainer.innerHTML = `<div class="placeholder-text">${t('drawer_no_events_yet')}</div>`;
      } else {
        timelineContainer.innerHTML = `<div class="placeholder-text" style="color: var(--neon-red);">${t('drawer_load_failed_cleaned')}</div>`;
      }
      return;
    }

    const data = await res.json();
    renderTimeline(data);

  } catch (err) {
    console.error('獲取會話細節失敗:', err);
    timelineContainer.innerHTML = `<div class="placeholder-text" style="color: var(--neon-red);">${t('drawer_load_failed')}</div>`;
  }
}

// 關閉抽屜
function closeDrawer() {
  document.getElementById('timeline-drawer').classList.remove('active');
}

// =========================================================================
// 渲染 Session 詳細時間軸 (Timeline) 內容
// =========================================================================
function renderTimeline(data) {
  const { metadata, timeline } = data;
  const timelineContainer = document.getElementById('timeline-items');
  timelineContainer.innerHTML = '';

  // 取得最終使用的基礎資訊（API 回傳優先，沒有則 fallback 到列表正確欄位）
  const finalCwd = metadata.cwd || currentSessionCwd || '-';
  const finalModel = metadata.selected_model || currentSessionModel || '-';

  // 更新 Metadata 區塊
  document.getElementById('meta-cwd').textContent = finalCwd;
  document.getElementById('meta-cwd').title = finalCwd;
  document.getElementById('meta-branch').textContent = metadata.git_branch || '-';
  document.getElementById('meta-model').textContent = finalModel;
  document.getElementById('meta-repo').textContent = metadata.repository || '-';
  document.getElementById('meta-repo').title = metadata.repository || '';

  const nicknameContainer = document.getElementById('drawer-meta-nickname-container');
  const roleContainer = document.getElementById('drawer-meta-role-container');

  if (metadata.agent_nickname) {
    document.getElementById('meta-nickname').textContent = metadata.agent_nickname;
    if (nicknameContainer) nicknameContainer.style.display = 'flex';
  } else {
    if (nicknameContainer) nicknameContainer.style.display = 'none';
  }

  if (metadata.agent_role) {
    document.getElementById('meta-role').textContent = metadata.agent_role;
    if (roleContainer) roleContainer.style.display = 'flex';
  } else {
    if (roleContainer) roleContainer.style.display = 'none';
  }

  const metaEffort = document.getElementById('meta-effort');
  if (metaEffort) {
    if (metadata.reasoning_effort) {
      metaEffort.textContent = metadata.reasoning_effort;
      metaEffort.style.display = 'inline-block';
    } else {
      metaEffort.style.display = 'none';
    }
  }

  // 取得最終使用的 Token 數據（若單一 session events 日誌無 token stats，則使用列表正確累積數據）
  const finalTotal = metadata.total_tokens || currentSessionTotalTokens || 0;
  const finalCache = metadata.total_cache_read_tokens || currentSessionCacheTokens || 0;
  const finalInput = metadata.total_input_tokens || currentSessionInputTokens || 0;
  const finalOutput = metadata.total_output_tokens || currentSessionOutputTokens || 0;
  const finalReasoning = metadata.total_reasoning_tokens || currentSessionReasoningTokens || 0;

  document.getElementById('meta-tokens').textContent = formatToken(finalTotal);
  document.getElementById('meta-cache').textContent = formatToken(finalCache);
  document.getElementById('meta-compaction').textContent = metadata.compaction_count || 0;
  document.getElementById('meta-input').textContent = formatToken(finalInput);
  document.getElementById('meta-output').textContent = formatToken(finalOutput);
  document.getElementById('meta-reasoning').textContent = formatToken(finalReasoning);

  if (!timeline || timeline.length === 0) {
    timelineContainer.innerHTML = `<div class="placeholder-text">${t('drawer_no_events')}</div>`;
    return;
  }

  // 渲染時間軸物件，使用單一回合序號進行對齊
  const hasUserPrompts = timeline.some(item => item.event_type === 'UserPrompt');
  let currentTurnNo = 1;
  let isFirstPrompt = true;

  timeline.forEach(item => {
    const timeStr = item.event_data.timestamp ? formatLocalTime(item.event_data.timestamp, true) : '';
    const div = document.createElement('div');
    div.className = 'timeline-item-wrapper';

    switch (item.event_type) {
      case 'UserPrompt': {
        if (!isFirstPrompt) {
          currentTurnNo++;
        }
        isFirstPrompt = false;
        const prompt = item.event_data.prompt;
        const turnNo = item.event_data.turn_no || currentTurnNo;
        
        let attachmentsHTML = '';
        if (item.event_data.attachments && item.event_data.attachments.length > 0) {
          attachmentsHTML = `<div class="bubble-attachments">`;
          item.event_data.attachments.forEach(att => {
            const path = att.filePath || att.path || '檔名未知';
            const basename = path.split('/').pop();
            const attType = att.type || 'file';
            attachmentsHTML += `
              <div class="attachment-badge" title="${escapeHtml(path)}">
                <strong>[${escapeHtml(attType)}]</strong> ${escapeHtml(basename)}
              </div>
            `;
          });
          attachmentsHTML += `</div>`;
        }

        div.innerHTML = `
          <div class="timeline-dot"></div>
          <div class="user-bubble">
            <div class="bubble-header">
              <div class="header-left">
                <span class="turn-no-badge">#${turnNo}</span>
                <span class="sender">${t('sender_user')}</span>
                <button class="header-collapse-btn" style="display: none; margin-left: 8px;">
                  <span class="btn-text">${t('collapse_reply')}</span> <span class="arrow">${iconMarkup('chevron-up', 'toggle-arrow-icon')}</span>
                </button>
              </div>
              <span class="time">${timeStr}</span>
            </div>
            <div class="prompt-content-wrapper">
              <div class="prompt-text collapsed">${escapeHtml(prompt)}</div>
              <button class="prompt-toggle-btn">
                <span class="btn-text">${t('expand_reply')}</span> <span class="arrow">${iconMarkup('chevron-down', 'toggle-arrow-icon')}</span>
              </button>
            </div>
            ${attachmentsHTML}
          </div>
        `;

        // 綁定提問摺疊按鈕事件
        const promptText = div.querySelector('.prompt-text');
        const promptToggleBtn = div.querySelector('.prompt-toggle-btn');
        const headerCollapseBtn = div.querySelector('.header-collapse-btn');

        const toggleCollapse = (collapse) => {
          if (collapse) {
            promptText.classList.remove('expanded');
            promptText.classList.add('collapsed');
            promptToggleBtn.classList.remove('expanded');
            promptToggleBtn.querySelector('.btn-text').textContent = t('expand_reply');
            setDisclosureIcon(promptToggleBtn.querySelector('.arrow'), false);
            if (headerCollapseBtn) headerCollapseBtn.style.display = 'none';
          } else {
            promptText.classList.remove('collapsed');
            promptText.classList.add('expanded');
            promptToggleBtn.classList.add('expanded');
            promptToggleBtn.querySelector('.btn-text').textContent = t('collapse_reply');
            setDisclosureIcon(promptToggleBtn.querySelector('.arrow'), true);
            if (headerCollapseBtn) headerCollapseBtn.style.display = 'inline-flex';
          }
        };

        if (promptText && promptToggleBtn) {
          promptToggleBtn.addEventListener('click', () => {
            const isCollapsed = promptText.classList.contains('collapsed');
            toggleCollapse(!isCollapsed);
          });
        }

        if (headerCollapseBtn) {
          headerCollapseBtn.addEventListener('click', () => {
            toggleCollapse(true); // Collapse it!
            
            // Smoothly scroll the container back into view
            div.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
          });
        }

        break;
      }

      case 'AssistantReply': {
        const replyMarkdown = item.event_data.reply;
        const model = item.event_data.model;
        const outTokens = item.event_data.output_tokens;
        const inTokens = item.event_data.input_tokens;
        const cacheReadTokens = item.event_data.cache_read_tokens;
        const cacheWriteTokens = item.event_data.cache_write_tokens;
        const reasoningTokens = item.event_data.reasoning_tokens;
        const totalTokens = item.event_data.total_tokens || ((inTokens || outTokens) ? ((inTokens || 0) + (outTokens || 0)) : null);
        const turnNo = item.event_data.turn_no || currentTurnNo;
        const reasoningEffort = item.event_data.reasoning_effort;
        const modelDisplay = reasoningEffort ? `${model} (${t('drawer_effort')}: ${reasoningEffort})` : model;

        const finalAssistantType = metadata.assistant_type || currentSessionAssistantType || currentAssistant;
        let senderLogoHtml = '';
        let senderNameText = 'AGENT';
        if (isSupportedAssistant(finalAssistantType)) {
          const meta = getAssistantMeta(finalAssistantType);
          senderLogoHtml = getAssistantLogoHtml(finalAssistantType);
          senderNameText = meta.senderName;
        }

        // 如果 content 為空但有 Tool 呼叫，代表助理正在使用工具
        let replyHtml = '';
        const toolRequests = item.event_data.tool_requests || [];
        const hasTools = toolRequests.length > 0;

        if (!replyMarkdown && hasTools) {
          replyHtml = `<span style="font-style: italic; color: var(--text-muted);">${t('thinking_tools')}</span>`;
        } else {
          replyHtml = renderSafeMarkdown(replyMarkdown || '');
        }

        // 建立詳細 Token 資訊區塊 (in, out, reasoning, cache, total)
        let tokenBadge = '';
        if (totalTokens || inTokens || outTokens || cacheReadTokens || reasoningTokens) {
          tokenBadge = `
            <div class="turn-token-stats">
              ${inTokens ? `<span class="token-badge input" title="輸入 Token (Input Tokens)">In: ${formatToken(inTokens)}</span>` : ''}
              ${outTokens ? `<span class="token-badge output" title="輸出 Token (Output Tokens)">Out: ${formatToken(outTokens)}</span>` : ''}
              ${reasoningTokens ? `<span class="token-badge reasoning" title="推理 Token (Reasoning Tokens)">Reasoning: ${formatToken(reasoningTokens)}</span>` : ''}
              ${cacheReadTokens ? `<span class="token-badge cache" title="快取讀取 Token (Cache Read Tokens)">Cache: ${formatToken(cacheReadTokens)}</span>` : ''}
              ${totalTokens ? `<span class="token-badge total" title="總 Token (Total Tokens)">Total: ${formatToken(totalTokens)}</span>` : ''}
            </div>
          `;
        }

        let copyButtonHtml = '';
        if (replyMarkdown) {
          copyButtonHtml = `
            <button class="copy-markdown-btn" title="${t('copy_markdown_title')}">
              <span class="btn-text">${t('copy_markdown')}</span>
            </button>
          `;
        }

        div.innerHTML = `
          <div class="timeline-dot"></div>
          <div class="assistant-bubble">
            <div class="bubble-header">
              <div class="header-left">
                <span class="turn-no-badge">#${turnNo}</span>
                <span class="sender">${senderLogoHtml} ${senderNameText} (${escapeHtml(modelDisplay)})</span>
              </div>
              <div style="display: flex; align-items: center; gap: 12px; flex-wrap: wrap;">
                ${copyButtonHtml}
                <span class="time">${timeStr}</span>
              </div>
            </div>
            ${tokenBadge}
            <div class="reply-content-wrapper">
              <div class="reply-content collapsed">${replyHtml}</div>
              <button class="reply-toggle-btn">
                <span class="btn-text">${t('expand_reply')}</span> <span class="arrow">${iconMarkup('chevron-down', 'toggle-arrow-icon')}</span>
              </button>
            </div>
          </div>
        `;

        // 綁定摺疊按鈕事件
        const replyContent = div.querySelector('.reply-content');
        const toggleBtn = div.querySelector('.reply-toggle-btn');
        if (replyContent && toggleBtn) {
          toggleBtn.addEventListener('click', () => {
            const isCollapsed = replyContent.classList.contains('collapsed');
            if (isCollapsed) {
              replyContent.classList.remove('collapsed');
              replyContent.classList.add('expanded');
              toggleBtn.classList.add('expanded');
              toggleBtn.querySelector('.btn-text').textContent = t('collapse_reply');
              setDisclosureIcon(toggleBtn.querySelector('.arrow'), true);
            } else {
              replyContent.classList.remove('expanded');
              replyContent.classList.add('collapsed');
              toggleBtn.classList.remove('expanded');
              toggleBtn.querySelector('.btn-text').textContent = t('expand_reply');
              setDisclosureIcon(toggleBtn.querySelector('.arrow'), false);
            }
          });
        }

        // 如果此助理訊息沒有調用任何 Tool，且此會話沒有使用者提問事件，則將回合序號遞增 1
        if (!hasTools && !hasUserPrompts) {
          currentTurnNo++;
        }

        // 綁定複製 Markdown 事件
        if (replyMarkdown) {
          const copyBtn = div.querySelector('.copy-markdown-btn');
          if (copyBtn) {
            copyBtn.addEventListener('click', () => {
              navigator.clipboard.writeText(replyMarkdown).then(() => {
                const btnTextEl = copyBtn.querySelector('.btn-text');
                const originalText = btnTextEl ? btnTextEl.textContent : 'Copy Markdown';
                if (btnTextEl) btnTextEl.textContent = t('copy_success');
                copyBtn.classList.add('copied');
                
                setTimeout(() => {
                  if (btnTextEl) btnTextEl.textContent = originalText;
                  copyBtn.classList.remove('copied');
                }, 2000);
              }).catch((err) => {
                console.error('Failed to copy text: ', err);
                showNotification(t('copy_failed'), 'error');
              });
            });
          }
        }

        break;
      }

      case 'ToolStep': {
        const toolName = item.event_data.tool_name;
        const args = item.event_data.arguments;
        const result = item.event_data.result;

        const isSuccess = result !== null && result !== undefined;
        const badgeClass = isSuccess ? 'badge success' : 'badge executing';
        const badgeText = isSuccess ? 'Success' : 'Executing';

        // 格式化 Args & Result 為 Pre 區塊
        const argsStr = stringifyToolValue(args, '{}');
        const argsBytes = getUtf8ByteLength(argsStr);
        
        let rawResultStr = '';
        if (result !== null && result !== undefined) {
          if (Object.prototype.hasOwnProperty.call(result, 'textResultForLlm')) {
            rawResultStr = stringifyToolValue(result.textResultForLlm);
          } else if (Object.prototype.hasOwnProperty.call(result, 'content')) {
            rawResultStr = stringifyToolValue(result.content);
          } else {
            rawResultStr = stringifyToolValue(result);
          }
        }
        const outputBytes = getUtf8ByteLength(rawResultStr);
        const resultStr = rawResultStr || t('no_returned_data');
        const argsBytesLabel = formatHumanBytes(argsBytes);
        const outputBytesLabel = formatHumanBytes(outputBytes);
        const argsBytesTitle = t('tool_args_bytes_title')
          .replace('{bytes}', argsBytesLabel)
          .replace('{raw}', formatNumber(argsBytes));
        const outputBytesTitle = t('tool_output_bytes_title')
          .replace('{bytes}', outputBytesLabel)
          .replace('{raw}', formatNumber(outputBytes));

        // 限制顯示長度，防止大日誌撐爆介面
        const truncatedResultStr = resultStr.length > 1500 ? resultStr.substring(0, 1500) + '\n' + t('data_truncated') : resultStr;

        div.innerHTML = `
          <div class="timeline-dot"></div>
          <div class="tool-step-bubble">
            <div class="tool-header">
              <div class="tool-info">
                <span class="tool-name">${escapeHtml(toolName)}</span>
                <span class="${badgeClass}">${badgeText}</span>
                <span class="badge data-size args" title="${escapeHtml(argsBytesTitle)}">${t('tool_args_size')}: ${escapeHtml(argsBytesLabel)}</span>
                <span class="badge data-size output" title="${escapeHtml(outputBytesTitle)}">${t('tool_output_size')}: ${escapeHtml(outputBytesLabel)}</span>
              </div>
              <span class="toggle-icon">${iconMarkup('chevron-right', 'tool-toggle-icon')}</span>
            </div>
            <div class="tool-details">
              <div class="detail-section">
                <span>${t('tool_arguments')}</span>
                <pre><code>${escapeHtml(argsStr)}</code></pre>
              </div>
              <div class="detail-section">
                <span>${t('tool_result')}</span>
                <pre><code>${escapeHtml(truncatedResultStr)}</code></pre>
              </div>
            </div>
          </div>
        `;

        // 綁定點擊展開事件
        const header = div.querySelector('.tool-header');
        header.addEventListener('click', () => {
          const bubble = header.closest('.tool-step-bubble');
          bubble.classList.toggle('expanded');
        });

        break;
      }

      case 'SystemStatus': {
        let message = item.event_data.message;
        if (message === '會話開始 (Session Started)') {
          message = t('session_started');
        } else if (message === '會話結束 (Session Ended)') {
          message = t('session_ended');
        } else if (message === '會話狀態壓縮完成 (Session Compaction Completed)') {
          message = t('session_compaction');
        }

        let statusLabel = 'System';
        if (item.event_data.status_type === 'session_compaction') {
          statusLabel = 'Compaction';
        }

        div.innerHTML = `
          <div class="timeline-dot"></div>
          <div class="system-bubble">
            <div class="system-badge">
              <span class="system-kind">${statusLabel}</span> ${escapeHtml(message)} <span class="time">${timeStr}</span>
            </div>
          </div>
        `;
        break;
      }
    }

    timelineContainer.appendChild(div);
  });
}

// =========================================================================
// Helpers / Utilities
// =========================================================================
function formatNumber(num) {
  if (num === null || num === undefined) return '-';
  return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

function getUtf8ByteLength(value) {
  if (value === null || value === undefined) return 0;
  return utf8TextEncoder.encode(String(value)).length;
}

function stringifyToolValue(value, fallback = '') {
  if (value === null || value === undefined) return fallback;
  if (typeof value === 'string') return value;
  const json = JSON.stringify(value, null, 2);
  return json === undefined ? String(value) : json;
}

function formatHumanBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${formatNumber(n)} B`;

  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = n / 1024;
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex++;
  }

  const formatted = value < 10 ? value.toFixed(1) : Math.round(value).toString();
  return `${formatted} ${units[unitIndex]}`;
}

function formatToken(num) {
  if (num === null || num === undefined) return '-';
  const n = Number(num);
  if (isNaN(n)) return '-';
  const thousand = 1000;
  const million = 1000000;
  const billion = 1000000000;
  const billionThreshold = 1024 * million;
  const formatCompactValue = (value) => {
    const rounded = Number(value.toFixed(1));
    return Number.isInteger(rounded) ? rounded.toString() : rounded.toFixed(1);
  };

  if (n > billionThreshold) {
    return `${(n / billion).toFixed(2)}b`;
  }
  if (n >= million) {
    return `${formatCompactValue(n / million)}m`;
  }
  if (n >= thousand) {
    return `${formatCompactValue(n / thousand)}k`;
  }
  return n.toString();
}

function calculatePercentage(part, total) {
  const denominator = Number(total) || 0;
  if (!denominator) return '0.00%';
  const percent = ((Number(part) || 0) / denominator) * 100;
  if (percent < 10) return `${percent.toFixed(2)}%`;
  if (percent < 100) return `${percent.toFixed(1)}%`;
  return `${Math.round(percent)}%`;
}

function getNetInputTokens(inputTokens, cacheReadTokens) {
  const input = Number(inputTokens) || 0;
  const cacheRead = Number(cacheReadTokens) || 0;
  return input >= cacheRead ? input - cacheRead : input;
}

function getDisplayedInputTokens(stats) {
  return getNetInputTokens(stats?.total_input_tokens, stats?.total_cache_read_tokens)
    + (Number(stats?.total_reasoning_tokens) || 0);
}

function formatDuration(ms) {
  if (ms === null || ms === undefined || ms === 0) return '-';
  if (ms < 1000) return `${ms}ms`;
  
  const totalSecs = ms / 1000;
  if (totalSecs < 60) {
    return `${totalSecs.toFixed(1)}s`;
  }
  
  const totalSecsInt = Math.floor(totalSecs);
  const hours = Math.floor(totalSecsInt / 3600);
  const minutes = Math.floor((totalSecsInt % 3600) / 60);
  const seconds = totalSecsInt % 60;
  
  const pad = (num) => String(num).padStart(2, '0');
  
  if (hours > 0) {
    return `${hours}:${pad(minutes)}:${pad(seconds)}`;
  } else {
    return `${minutes}:${pad(seconds)}`;
  }
}

function formatLocalTime(isoString, includeSeconds = true) {
  if (!isoString) return '';
  try {
    const date = new Date(isoString);
    if (isNaN(date.getTime())) return '';
    const pad = (num) => String(num).padStart(2, '0');
    const hours = pad(date.getHours());
    const minutes = pad(date.getMinutes());
    if (includeSeconds) {
      const seconds = pad(date.getSeconds());
      return `${hours}:${minutes}:${seconds}`;
    }
    return `${hours}:${minutes}`;
  } catch (err) {
    return '';
  }
}

function escapeHtml(unsafe) {
  if (!unsafe) return '';
  return unsafe
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// =========================================================================
// API 呼叫: 載入月份清單
// =========================================================================
async function fetchMonths(selectedMonth = null) {
  try {
    const res = await fetch(`/api/${currentAssistant}/months`);
    const data = await res.json();
    
    const monthSelect = document.getElementById('month-select');
    const urlMonth = getUrlDateForTab('monthly');
    const targetMonth = selectedMonth || urlMonth || monthSelect.value;
    
    monthSelect.innerHTML = '';

    if (!data.months || data.months.length === 0) {
      monthSelect.innerHTML = `<option value="" disabled selected>${t('no_month_logs')}</option>`;
      return;
    }

    let monthToLoad = data.months[0];
    let hasSelected = false;

    data.months.forEach((month) => {
      const opt = document.createElement('option');
      opt.value = month;
      opt.textContent = month;
      if (targetMonth && month === targetMonth) {
        opt.selected = true;
        monthToLoad = month;
        hasSelected = true;
      }
      monthSelect.appendChild(opt);
    });

    if (!hasSelected) {
      if (monthSelect.options.length > 0) {
        monthSelect.options[0].selected = true;
      }
    }

    if (activeTab === 'monthly') {
      await loadMonthlyData(monthToLoad);
    }

  } catch (err) {
    console.error('獲取月份清單失敗:', err);
    showNotification(t('load_failed'), 'error');
  }
}

async function reloadMonthlyData() {
  const monthSelect = document.getElementById('month-select');
  const selectedMonth = monthSelect.value;
  await fetchMonths(selectedMonth);
}

// =========================================================================
// API 呼叫: 載入年份清單
// =========================================================================
async function fetchYears(selectedYear = null) {
  try {
    const res = await fetch(`/api/${currentAssistant}/years`);
    const data = await res.json();
    
    const yearSelect = document.getElementById('year-select');
    if (!yearSelect) return;
    const urlYear = getUrlDateForTab('yearly');
    const targetYear = selectedYear || urlYear || yearSelect.value;
    
    yearSelect.innerHTML = '';

    if (!data.years || data.years.length === 0) {
      yearSelect.innerHTML = `<option value="" disabled selected>${t('no_year_logs')}</option>`;
      return;
    }

    let yearToLoad = data.years[0];
    let hasSelected = false;

    data.years.forEach((year) => {
      const opt = document.createElement('option');
      opt.value = year;
      opt.textContent = year;
      if (targetYear && year === targetYear) {
        opt.selected = true;
        yearToLoad = year;
        hasSelected = true;
      }
      yearSelect.appendChild(opt);
    });

    if (!hasSelected) {
      if (yearSelect.options.length > 0) {
        yearSelect.options[0].selected = true;
      }
    }

    if (activeTab === 'yearly') {
      await loadYearlyData(yearToLoad);
    }

  } catch (err) {
    console.error('獲取年份清單失敗:', err);
    showNotification(t('load_failed'), 'error');
  }
}

async function reloadYearlyData() {
  const yearSelect = document.getElementById('year-select');
  if (yearSelect) {
    const selectedYear = yearSelect.value;
    await fetchYears(selectedYear);
  }
}

// =========================================================================
// API 呼叫: 載入單年彙整數據
// =========================================================================
async function loadYearlyData(year) {
  if (!year || year === 'undefined' || year === 'null') {
    return;
  }
  updateUrlParams();
  try {
    setTitleMarkup('sync', year);

    const res = await fetch(`/api/${currentAssistant}/yearly/${year}`);
    if (res.status === 404) {
      showNotification(t('year_not_found'), 'error');
      return;
    }
    
    const data = await res.json();
    toggleEmptyState(false);
    renderYearlyDashboard(data);

  } catch (err) {
    console.error('載入年份彙整失敗:', err);
    showNotification(t('yearly_load_failed'), 'error');
  }
}

// =========================================================================
// 渲染年報看板數據
// =========================================================================
function renderYearlyDashboard(data) {
  currentYearlyData = data;
  const { year, summary, monthly_breakdown, models, projects, agent_breakdown } = data;

  // 1. 更新標題
  setTitleMarkup('calendar', year);

  // 2. 更新指標卡片
  const activeAgents = getActiveAgents();
  const isMulti = activeAgents.length > 1;
  const yearlyInputTokens = getNetInputTokens(summary.total_input_tokens, summary.total_cache_read_tokens);

  if (!isMulti) {
    const totalTokensEl = document.getElementById('yearly-stat-total-tokens');
    const inputTokensEl = document.getElementById('yearly-stat-input-tokens');
    const outputTokensEl = document.getElementById('yearly-stat-output-tokens');
    const sessionsEl = document.getElementById('yearly-stat-sessions');
    const totalCostEl = document.getElementById('yearly-stat-total-cost');

    if (totalTokensEl) totalTokensEl.textContent = formatToken(summary.total_tokens);
    if (inputTokensEl) inputTokensEl.textContent = formatToken(yearlyInputTokens);
    if (outputTokensEl) outputTokensEl.textContent = formatToken(summary.total_output_tokens);
    if (sessionsEl) sessionsEl.textContent = summary.total_sessions;
    if (totalCostEl) totalCostEl.textContent = formatCost(summary.total_cost_usd || 0);
  } else {
    renderYearlyMetricValue('yearly-stat-total-tokens', a => a.total_tokens, formatToken, agent_breakdown, activeAgents);
    renderYearlyMetricValue('yearly-stat-input-tokens', a => getNetInputTokens(a.total_input_tokens, a.total_cache_read_tokens), formatToken, agent_breakdown, activeAgents);
    renderYearlyMetricValue('yearly-stat-output-tokens', a => a.total_output_tokens, formatToken, agent_breakdown, activeAgents);
    renderYearlyMetricValue('yearly-stat-total-cost', a => a.total_cost_usd, formatCost, agent_breakdown, activeAgents);
    
    // For sessions: show individual session count list
    let sessionsHtml = '<div class="stat-value-list">';
    activeAgents.forEach(a => {
      const meta = getAssistantMeta(a);
      let logoUrl = meta.logo;
      let displayName = meta.label;
      const val = (agent_breakdown && agent_breakdown[a]) ? agent_breakdown[a].total_sessions : 0;
      sessionsHtml += `
        <div class="stat-value-item">
          <span class="agent-name" title="${displayName}"><img class="badge-logo" src="${logoUrl}" alt="${displayName}" /></span>
          <span class="val">${formatNumber(val)}</span>
        </div>
      `;
    });
    sessionsHtml += '</div>';
    const sessionsEl = document.getElementById('yearly-stat-sessions');
    if (sessionsEl) sessionsEl.innerHTML = sessionsHtml;
  }

  const statCacheRead = document.getElementById('yearly-stat-cache-read');
  const statInputPct = document.getElementById('yearly-stat-input-pct');
  const statOutputPct = document.getElementById('yearly-stat-output-pct');
  const statRequests = document.getElementById('yearly-stat-requests');

  if (isMulti) {
    if (statCacheRead) statCacheRead.classList.add('hidden');
    if (statInputPct) statInputPct.classList.add('hidden');
    if (statOutputPct) statOutputPct.classList.add('hidden');
    if (statRequests) statRequests.classList.add('hidden');
  } else {
    if (statCacheRead) {
      statCacheRead.classList.remove('hidden');
      statCacheRead.textContent = `${t('cache_read_label')}: ${formatToken(summary.total_cache_read_tokens)} (${calculatePercentage(summary.total_cache_read_tokens, summary.total_tokens)})`;
    }
    if (statInputPct) {
      statInputPct.classList.remove('hidden');
      statInputPct.textContent = `${t('ratio_label')}: ${calculatePercentage(yearlyInputTokens, summary.total_tokens)}`;
    }
    if (statOutputPct) {
      statOutputPct.classList.remove('hidden');
      statOutputPct.textContent = `${t('ratio_label')}: ${calculatePercentage(summary.total_output_tokens, summary.total_tokens)}`;
    }
    if (statRequests) {
      statRequests.classList.remove('hidden');
      statRequests.textContent = t('yearly_requests_count').replace('{count}', formatNumber(summary.total_requests));
    }
  }

  // 3. 繪製單年每月趨勢圖
  renderYearlyChart(monthly_breakdown);

  // 4. 渲染最常活動專案列表
  renderYearlyProjectsTable(projects);

  // 5. 渲染模型佔比列表
  renderYearlyModelsTable(models);

  // 6. 渲染當年每月彙總列表
  yearlyMonthlySortColumn = 'month';
  yearlyMonthlySortDirection = 'desc';
  sortAndRenderYearlyMonthlyTable();
}

function renderYearlyMetricValue(elementId, getter, formatter, agentBreakdown, activeAgents) {
  let html = '<div class="stat-value-list">';
  activeAgents.forEach(a => {
    const meta = getAssistantMeta(a);
      let logoUrl = meta.logo;
      let displayName = meta.label;
    const val = (agentBreakdown && agentBreakdown[a]) ? getter(agentBreakdown[a]) : 0;
    html += `
      <div class="stat-value-item">
        <span class="agent-name" title="${displayName}"><img class="badge-logo" src="${logoUrl}" alt="${displayName}" /></span>
        <span class="val">${formatter(val)}</span>
      </div>
    `;
  });
  html += '</div>';
  const el = document.getElementById(elementId);
  if (el) el.innerHTML = html;
}

// =========================================================================
// 渲染單年每月 Token 與 Session 趨勢圖
// =========================================================================
function renderYearlyChart(monthlyBreakdown) {
  currentYearlyBreakdown = monthlyBreakdown;
  currentYearlyChartData = [...monthlyBreakdown];
  const canvas = document.getElementById('yearlyTokenChart');
  if (!canvas) return;

  const labels = monthlyBreakdown.map(entry => entry.month);
  const tokenData = monthlyBreakdown.map(entry => entry.total_tokens);
  const cacheData = monthlyBreakdown.map(entry => entry.total_cache_read_tokens || 0);
  const sessionData = monthlyBreakdown.map(entry => entry.sessions_count);

  if (yearlyChartInstance) {
    yearlyChartInstance.data.labels = labels;
    yearlyChartInstance.data.datasets[0].label = t('chart_yearly_token_label');
    yearlyChartInstance.data.datasets[1].label = t('chart_cache_label');
    yearlyChartInstance.data.datasets[2].label = t('chart_yearly_session_label');
    yearlyChartInstance.data.datasets[0].data = tokenData;
    yearlyChartInstance.data.datasets[1].data = cacheData;
    yearlyChartInstance.data.datasets[2].data = sessionData;
    if (yearlyChartInstance.options.scales && yearlyChartInstance.options.scales.y && yearlyChartInstance.options.scales.y.title) {
      yearlyChartInstance.options.scales.y.title.text = t('col_total');
    }
    if (yearlyChartInstance.options.scales && yearlyChartInstance.options.scales.y1 && yearlyChartInstance.options.scales.y1.title) {
      yearlyChartInstance.options.scales.y1.title.text = t('col_sessions_count');
    }
    yearlyChartInstance.update();
    return;
  }

  yearlyChartInstance = new Chart(canvas, {
    type: 'bar',
    data: {
      labels: labels,
      datasets: [
        {
          label: t('chart_yearly_token_label'),
          data: tokenData,
          backgroundColor: chartPalette.tokenFill,
          borderColor: chartPalette.tokenStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_cache_label'),
          data: cacheData,
          backgroundColor: chartPalette.cacheFill,
          borderColor: chartPalette.cacheStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_yearly_session_label'),
          data: sessionData,
          type: 'line',
          borderColor: chartPalette.trendStroke,
          backgroundColor: chartPalette.trendFill,
          borderWidth: 3,
          pointBackgroundColor: chartPalette.trendStroke,
          pointBorderColor: '#fff',
          pointBorderWidth: 1.5,
          pointRadius: 4,
          pointHoverRadius: 6,
          yAxisID: 'y1',
          tension: 0.35,
        }
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      onClick: (event, elements) => {
        if (elements && elements.length > 0) {
          const index = elements[0].index;
          const selectedEntry = currentYearlyChartData[index];
          if (selectedEntry && selectedEntry.month) {
            switchToMonthlyMonth(selectedEntry.month);
          }
        }
      },
      onHover: (event, activeElements) => {
        canvas.style.cursor = activeElements.length ? 'pointer' : 'default';
      },
      interaction: {
        mode: 'index',
        intersect: false,
      },
      plugins: {
        legend: {
          position: 'top',
          labels: {
            color: '#94a3b8',
            font: { family: chartFontFamily, size: 12 }
          }
        },
        tooltip: {
          backgroundColor: 'rgba(15, 23, 42, 0.92)',
          borderColor: 'rgba(255, 255, 255, 0.08)',
          borderWidth: 1,
          titleColor: chartPalette.tokenStroke,
          titleFont: { size: 14, weight: 'bold' },
          bodyFont: { size: 13 },
          padding: 12,
          cornerRadius: 8,
          callbacks: {
            label: function(context) {
              let label = context.dataset.label || '';
              if (label) {
                label += ': ';
              }
              if (context.dataset.type === 'line') {
                label += formatNumber(context.parsed.y) + ' Sessions';
              } else {
                label += formatToken(context.parsed.y);
              }
              return label;
            }
          }
        }
      },
      scales: {
        x: {
          grid: { display: false },
          ticks: { color: '#94a3b8', font: { size: 11 } }
        },
        y: {
          type: 'linear',
          display: true,
          position: 'left',
          grid: { color: 'rgba(255, 255, 255, 0.04)' },
          ticks: {
            color: '#94a3b8',
            font: { size: 11 },
            callback: value => formatToken(value)
          },
          title: {
            display: true,
            text: t('col_total'),
            color: '#94a3b8',
            font: { size: 11 }
          }
        },
        y1: {
          type: 'linear',
          display: true,
          position: 'right',
          grid: { drawOnChartArea: false },
          ticks: {
            color: '#94a3b8',
            font: { size: 11 },
            callback: value => formatNumber(value)
          },
          title: {
            display: true,
            text: t('col_sessions_count'),
            color: '#94a3b8',
            font: { size: 11 }
          }
        }
      }
    }
  });

  const currentTheme = document.documentElement.getAttribute('data-theme') || 'dark';
  updateChartsTheme(currentTheme);
}

// =========================================================================
// 渲染年度專案列表 Table
// =========================================================================
function renderYearlyProjectsTable(projects) {
  const tbody = document.getElementById('yearly-projects-body');
  if (!tbody) return;
  tbody.innerHTML = '';

  if (projects.length === 0) {
    tbody.innerHTML = `<tr><td colspan="4" class="placeholder-text">${t('placeholder_no_projects')}</td></tr>`;
    return;
  }

  // 僅取前 15 名
  const displayProjects = projects.slice(0, 15);

  displayProjects.forEach((p, idx) => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'default';

    tr.innerHTML = `
      <td style="text-align: center;"><span class="badge ${idx < 3 ? 'highlight' : ''}">${idx + 1}</span></td>
      <td class="cwd-cell" title="${escapeHtml(p.cwd)}" style="max-width: 250px;">${escapeHtml(p.cwd)}</td>
      <td><span class="badge">${p.sessions_count} Sessions</span></td>
      <td style="font-weight: 700; color: var(--accent-cyan);">
        ${formatToken(p.total_tokens)}
        ${p.total_cache_read_tokens ? `<div style="font-size: 0.72rem; font-weight: normal; color: #a5b4fc; margin-top: 3px;" title="${t('chart_cache_label')}">${t('cache_prefix')}${formatToken(p.total_cache_read_tokens)}</div>` : ''}
      </td>
    `;
    tbody.appendChild(tr);
  });
}

// =========================================================================
// 渲染年度模型佔比列表 Table
// =========================================================================
function renderYearlyModelsTable(models) {
  const tbody = document.getElementById('yearly-models-body');
  if (!tbody) return;
  tbody.innerHTML = '';

  if (models.length === 0) {
    tbody.innerHTML = `<tr><td colspan="5" class="placeholder-text">${t('placeholder_no_models')}</td></tr>`;
    return;
  }

  models.forEach((m, idx) => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'default';

    tr.innerHTML = `
      <td style="text-align: center;"><span class="badge ${idx < 3 ? 'highlight' : ''}">${idx + 1}</span></td>
      <td><span class="badge highlight">${escapeHtml(m.model)}</span></td>
      <td><span class="badge">${m.sessions_count} Sessions</span></td>
      <td style="font-weight: 700; color: var(--accent-purple);">
        ${formatToken(m.total_tokens)}
        ${m.total_cache_read_tokens ? `<div style="font-size: 0.72rem; font-weight: normal; color: #a5b4fc; margin-top: 3px;" title="${t('chart_cache_label')}">${t('cache_prefix')}${formatToken(m.total_cache_read_tokens)}</div>` : ''}
      </td>
      <td style="font-weight: 700; color: var(--neon-gold);">${formatCost(m.cost_usd || 0)}</td>
    `;
    tbody.appendChild(tr);
  });
}

// =========================================================================
// 渲染當年每月彙總 Table
// =========================================================================
function renderYearlyMonthlySummaryTable(monthlyBreakdown) {
  const tbody = document.getElementById('yearly-monthly-summary-body');
  if (!tbody) return;
  tbody.innerHTML = '';

  if (!monthlyBreakdown || monthlyBreakdown.length === 0) {
    tbody.innerHTML = `<tr><td colspan="7" class="placeholder-text">${t('placeholder_no_monthly_summary')}</td></tr>`;
    return;
  }

  monthlyBreakdown.forEach(entry => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'pointer';
    
    // 點選整列可跳轉並帶入該月份查詢
    tr.addEventListener('click', () => {
      switchToMonthlyMonth(entry.month);
    });

    tr.innerHTML = `
      <td style="font-weight: 600; color: var(--accent-cyan);">${escapeHtml(entry.month)}</td>
      <td style="color: var(--text-secondary);">${formatToken(entry.total_input_tokens || 0)}</td>
      <td style="color: var(--text-secondary);">${formatToken(entry.total_output_tokens || 0)}</td>
      <td style="color: #aeb9c8;">${formatToken(entry.total_reasoning_tokens || 0)}</td>
      <td style="color: #34d399;">${formatToken(entry.total_cache_read_tokens || 0)}</td>
      <td style="font-weight: 700; color: #fbbf24;">${formatToken(entry.total_tokens)}</td>
      <td style="font-weight: 700; color: var(--neon-gold);">${formatCost(entry.cost_usd || 0)}</td>
    `;
    tbody.appendChild(tr);
  });
}

function switchToMonthlyMonth(monthStr) {
  const monthSelect = document.getElementById('month-select');
  if (!monthSelect) return;
  
  // 檢查 option 是否存在，若不存在則動態加入
  let exists = false;
  for (let i = 0; i < monthSelect.options.length; i++) {
    if (monthSelect.options[i].value === monthStr) {
      exists = true;
      break;
    }
  }
  if (!exists) {
    const opt = document.createElement('option');
    opt.value = monthStr;
    opt.textContent = monthStr;
    let inserted = false;
    for (let i = 0; i < monthSelect.options.length; i++) {
      if (monthSelect.options[i].value < monthStr) {
        monthSelect.insertBefore(opt, monthSelect.options[i]);
        inserted = true;
        break;
      }
    }
    if (!inserted) {
      monthSelect.appendChild(opt);
    }
  }
  
  monthSelect.value = monthStr;
  switchTab('monthly');
}

function sortAndRenderYearlyMonthlyTable() {
  if (!currentYearlyBreakdown || currentYearlyBreakdown.length === 0) {
    renderYearlyMonthlySummaryTable([]);
    return;
  }

  currentYearlyBreakdown.sort((a, b) => {
    let valA, valB;
    if (yearlyMonthlySortColumn === 'month') {
      valA = a.month;
      valB = b.month;
    } else {
      const keyMap = {
        'input': 'total_input_tokens',
        'output': 'total_output_tokens',
        'reasoning': 'total_reasoning_tokens',
        'cache': 'total_cache_read_tokens',
        'total': 'total_tokens',
        'cost': 'cost_usd'
      };
      const field = keyMap[yearlyMonthlySortColumn] || yearlyMonthlySortColumn;
      valA = a[field];
      valB = b[field];
    }

    if (valA === undefined || valA === null) valA = 0;
    if (valB === undefined || valB === null) valB = 0;

    if (typeof valA === 'string' && typeof valB === 'string') {
      return yearlyMonthlySortDirection === 'asc' 
        ? valA.localeCompare(valB) 
        : valB.localeCompare(valA);
    }

    return yearlyMonthlySortDirection === 'asc' ? valA - valB : valB - valA;
  });

  renderYearlyMonthlySummaryTable(currentYearlyBreakdown);
  updateYearlySortHeadersUI();
}

function updateYearlySortHeadersUI() {
  const headers = document.querySelectorAll('.premium-table th.sortable[data-table="yearly"]');
  headers.forEach(th => {
    const column = th.getAttribute('data-sort');
    const icon = th.querySelector('.sort-icon');
    if (!icon) return;

    th.classList.remove('sorted-asc', 'sorted-desc');
    
    if (column === yearlyMonthlySortColumn) {
      if (yearlyMonthlySortDirection === 'asc') {
        th.classList.add('sorted-asc');
        icon.innerHTML = iconMarkup('chevron-up', 'sort-glyph');
      } else {
        th.classList.add('sorted-desc');
        icon.innerHTML = iconMarkup('chevron-down', 'sort-glyph');
      }
    } else {
      icon.innerHTML = `<span class="sort-icon-placeholder">${iconMarkup('chevron-up', 'sort-glyph')}${iconMarkup('chevron-down', 'sort-glyph')}</span>`;
    }
  });
}

// =========================================================================
// API 呼叫: 載入單月彙整數據
// =========================================================================
async function loadMonthlyData(month) {
  if (!month || month === 'undefined' || month === 'null') {
    return;
  }
  updateUrlParams();
  try {
    setTitleMarkup('sync', month);

    const res = await fetch(`/api/${currentAssistant}/monthly/${month}`);
    if (res.status === 404) {
      showNotification(t('month_not_found'), 'error');
      return;
    }
    
    const data = await res.json();
    toggleEmptyState(false);
    renderMonthlyDashboard(data);

  } catch (err) {
    console.error('載入月份彙整失敗:', err);
    showNotification(t('monthly_load_failed'), 'error');
  }
}

// =========================================================================
// 渲染月報看板數據
// =========================================================================
function renderMonthlyDashboard(data) {
  currentMonthlyData = data;
  const { year_month, summary, daily_breakdown, models, projects, agent_breakdown } = data;

  // 1. 更新標題
  setTitleMarkup('calendar', year_month);

  // 2. 更新指標卡片
  const activeAgents = getActiveAgents();
  const isMulti = activeAgents.length > 1;
  const monthlyInputTokens = getNetInputTokens(summary.total_input_tokens, summary.total_cache_read_tokens);

  if (!isMulti) {
    document.getElementById('monthly-stat-total-tokens').textContent = formatToken(summary.total_tokens);
    document.getElementById('monthly-stat-input-tokens').textContent = formatToken(monthlyInputTokens);
    document.getElementById('monthly-stat-cache-input-tokens').textContent = formatToken(summary.total_cache_read_tokens || 0);
    document.getElementById('monthly-stat-output-tokens').textContent = formatToken(summary.total_output_tokens);
    document.getElementById('monthly-stat-total-cost').textContent = formatCost(summary.total_cost_usd || 0);
  } else {
    renderMonthlyMetricValue('monthly-stat-total-tokens', a => a.total_tokens, formatToken, agent_breakdown, activeAgents);
    renderMonthlyMetricValue('monthly-stat-input-tokens', a => getNetInputTokens(a.total_input_tokens, a.total_cache_read_tokens), formatToken, agent_breakdown, activeAgents);
    renderMonthlyMetricValue('monthly-stat-cache-input-tokens', a => a.total_cache_read_tokens || 0, formatToken, agent_breakdown, activeAgents);
    renderMonthlyMetricValue('monthly-stat-output-tokens', a => a.total_output_tokens, formatToken, agent_breakdown, activeAgents);
    renderMonthlyMetricValue('monthly-stat-total-cost', a => a.total_cost_usd, formatCost, agent_breakdown, activeAgents);
  }

  const statInputPct = document.getElementById('monthly-stat-input-pct');
  const statCacheInputPct = document.getElementById('monthly-stat-cache-input-pct');
  const statOutputPct = document.getElementById('monthly-stat-output-pct');
  const chartTotalSessions = document.getElementById('monthly-chart-total-sessions');

  if (chartTotalSessions) {
    chartTotalSessions.textContent = formatNumber(summary.total_sessions || 0);
  }

  if (isMulti) {
    if (statInputPct) statInputPct.classList.add('hidden');
    if (statCacheInputPct) statCacheInputPct.classList.add('hidden');
    if (statOutputPct) statOutputPct.classList.add('hidden');
  } else {
    if (statInputPct) {
      statInputPct.classList.remove('hidden');
      statInputPct.textContent = `${t('ratio_label')}: ${calculatePercentage(monthlyInputTokens, summary.total_tokens)}`;
    }
    if (statCacheInputPct) {
      statCacheInputPct.classList.remove('hidden');
      statCacheInputPct.textContent = `${t('ratio_label')}: ${calculatePercentage(summary.total_cache_read_tokens, summary.total_tokens)}`;
    }
    if (statOutputPct) {
      statOutputPct.classList.remove('hidden');
      statOutputPct.textContent = `${t('ratio_label')}: ${calculatePercentage(summary.total_output_tokens, summary.total_tokens)}`;
    }
  }

  // 3. 繪製單月每日趨勢圖
  renderMonthlyChart(daily_breakdown);

  // 4. 渲染最常活動專案列表
  renderMonthlyProjectsTable(projects);

  // 5. 渲染模型佔比列表
  renderMonthlyModelsTable(models);

  // 6. 渲染當月每日彙總列表
  monthlyDailySortColumn = 'date';
  monthlyDailySortDirection = 'desc';
  sortAndRenderMonthlyDailyTable();
}

// =========================================================================
// 渲染單月每日 Token 與 Session 趨勢圖
// =========================================================================
function renderMonthlyChart(dailyBreakdown) {
  currentMonthlyBreakdown = dailyBreakdown;
  currentMonthlyChartData = [...dailyBreakdown];
  const canvas = document.getElementById('monthlyTokenChart');

  // 提取標籤與數據
  const labels = dailyBreakdown.map(entry => entry.date.substring(5)); // 只顯示 MM-DD
  const tokenData = dailyBreakdown.map(entry => entry.total_tokens);
  const cacheData = dailyBreakdown.map(entry => entry.total_cache_read_tokens || 0);
  const sessionData = dailyBreakdown.map(entry => entry.total_sessions);

  // 若圖表已存在，則動態更新數據以達到平滑變動效果
  if (monthlyChartInstance) {
    monthlyChartInstance.data.labels = labels;
    monthlyChartInstance.data.datasets[0].label = t('chart_monthly_token_label');
    monthlyChartInstance.data.datasets[1].label = t('chart_cache_label');
    monthlyChartInstance.data.datasets[2].label = t('chart_monthly_session_label');
    monthlyChartInstance.data.datasets[0].data = tokenData;
    monthlyChartInstance.data.datasets[1].data = cacheData;
    monthlyChartInstance.data.datasets[2].data = sessionData;
    if (monthlyChartInstance.options.scales && monthlyChartInstance.options.scales.y && monthlyChartInstance.options.scales.y.title) {
      monthlyChartInstance.options.scales.y.title.text = t('col_total');
    }
    if (monthlyChartInstance.options.scales && monthlyChartInstance.options.scales.y1 && monthlyChartInstance.options.scales.y1.title) {
      monthlyChartInstance.options.scales.y1.title.text = t('col_sessions_count');
    }
    monthlyChartInstance.update();
    return;
  }

  monthlyChartInstance = new Chart(canvas, {
    type: 'bar',
    data: {
      labels: labels,
      datasets: [
        {
          label: t('chart_monthly_token_label'),
          data: tokenData,
          backgroundColor: chartPalette.tokenFill,
          borderColor: chartPalette.tokenStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_cache_label'),
          data: cacheData,
          backgroundColor: chartPalette.cacheFill,
          borderColor: chartPalette.cacheStroke,
          borderWidth: 1.5,
          borderRadius: 6,
          yAxisID: 'y',
          grouped: false,
          barPercentage: 0.8,
        },
        {
          label: t('chart_monthly_session_label'),
          data: sessionData,
          type: 'line',
          borderColor: chartPalette.trendStroke,
          backgroundColor: chartPalette.trendFill,
          borderWidth: 2,
          pointBackgroundColor: chartPalette.trendStroke,
          pointRadius: 4,
          tension: 0.2,
          yAxisID: 'y1',
        }
      ]
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      onClick: (event, elements) => {
        if (elements && elements.length > 0) {
          const index = elements[0].index;
          const selectedEntry = currentMonthlyChartData[index];
          if (selectedEntry && selectedEntry.date) {
            switchToDailyDate(selectedEntry.date);
          }
        }
      },
      onHover: (event, activeElements) => {
        canvas.style.cursor = activeElements.length ? 'pointer' : 'default';
      },
      plugins: {
        legend: {
          labels: {
            color: '#f3f4f6',
            font: {
              family: chartFontFamily
            }
          }
        },
        tooltip: {
          padding: 12,
          backgroundColor: 'rgba(15, 18, 29, 0.95)',
          titleColor: chartPalette.tokenStroke,
          bodyColor: '#f3f4f6',
          borderColor: 'rgba(255, 255, 255, 0.1)',
          borderWidth: 1,
          callbacks: {
            label: (context) => {
              const label = context.dataset.label || '';
              const value = context.parsed.y;
              if (label.includes('Token')) {
                return `${label}: ${formatToken(value)} (${formatNumber(value)})`;
              }
              return `${label}: ${formatNumber(value)}`;
            }
          }
        }
      },
      scales: {
        x: {
          stacked: false,
          grid: {
            color: 'rgba(255, 255, 255, 0.05)'
          },
          ticks: {
            color: '#9ca3af',
            font: {
              size: 10
            }
          }
        },
        y: {
          stacked: false,
          type: 'linear',
          position: 'left',
          grid: {
            color: 'rgba(255, 255, 255, 0.05)'
          },
          ticks: {
            color: '#9ca3af',
            callback: (value) => formatToken(value)
          },
          title: {
            display: true,
            text: t('col_total'),
            color: '#f3f4f6'
          }
        },
        y1: {
          stacked: false,
          type: 'linear',
          position: 'right',
          grid: {
            drawOnChartArea: false,
          },
          ticks: {
            color: '#9ca3af',
            stepSize: 1
          },
          title: {
            display: true,
            text: t('col_sessions_count')
          }
        }
      }
    }
  });

  // 根據當前主題更新圖表樣式
  const currentTheme = document.documentElement.getAttribute('data-theme') || 'dark';
  updateChartsTheme(currentTheme);
}

// =========================================================================
// 渲染最常活動專案列表 Table
// =========================================================================
function renderMonthlyProjectsTable(projects) {
  const tbody = document.getElementById('monthly-projects-body');
  tbody.innerHTML = '';

  if (projects.length === 0) {
    tbody.innerHTML = `<tr><td colspan="4" class="placeholder-text">${t('placeholder_no_projects')}</td></tr>`;
    return;
  }

  // 僅取前 15 名
  const displayProjects = projects.slice(0, 15);

  displayProjects.forEach((p, idx) => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'default';

    tr.innerHTML = `
      <td style="text-align: center;"><span class="badge ${idx < 3 ? 'highlight' : ''}">${idx + 1}</span></td>
      <td class="cwd-cell" title="${escapeHtml(p.cwd)}" style="max-width: 250px;">${escapeHtml(p.cwd)}</td>
      <td><span class="badge">${p.sessions_count} Sessions</span></td>
      <td style="font-weight: 700; color: var(--accent-cyan);">
        ${formatToken(p.total_tokens)}
        ${p.total_cache_read_tokens ? `<div style="font-size: 0.72rem; font-weight: normal; color: #a5b4fc; margin-top: 3px;" title="${t('chart_cache_label')}">${t('cache_prefix')}${formatToken(p.total_cache_read_tokens)}</div>` : ''}
      </td>
    `;
    tbody.appendChild(tr);
  });
}

// =========================================================================
// 渲染模型佔比列表 Table
// =========================================================================
function renderMonthlyModelsTable(models) {
  const tbody = document.getElementById('monthly-models-body');
  tbody.innerHTML = '';

  if (models.length === 0) {
    tbody.innerHTML = `<tr><td colspan="5" class="placeholder-text">${t('placeholder_no_models')}</td></tr>`;
    return;
  }

  models.forEach((m, idx) => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'default';

    tr.innerHTML = `
      <td style="text-align: center;"><span class="badge ${idx < 3 ? 'highlight' : ''}">${idx + 1}</span></td>
      <td><span class="badge highlight">${escapeHtml(m.model)}</span></td>
      <td><span class="badge">${m.sessions_count} Sessions</span></td>
      <td style="font-weight: 700; color: var(--accent-purple);">
        ${formatToken(m.total_tokens)}
        ${m.total_cache_read_tokens ? `<div style="font-size: 0.72rem; font-weight: normal; color: #a5b4fc; margin-top: 3px;" title="${t('chart_cache_label')}">${t('cache_prefix')}${formatToken(m.total_cache_read_tokens)}</div>` : ''}
      </td>
      <td style="font-weight: 700; color: var(--neon-gold);">${formatCost(m.cost_usd || 0)}</td>
    `;
    tbody.appendChild(tr);
  });
}

// =========================================================================
// 渲染當月每日彙總 Table
// =========================================================================
function renderMonthlyDailySummaryTable(dailyBreakdown) {
  const tbody = document.getElementById('monthly-daily-summary-body');
  tbody.innerHTML = '';

  if (!dailyBreakdown || dailyBreakdown.length === 0) {
    tbody.innerHTML = `<tr><td colspan="7" class="placeholder-text">${t('placeholder_no_daily_summary')}</td></tr>`;
    return;
  }

  dailyBreakdown.forEach(entry => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'pointer';
    
    // 點選整列可跳轉並帶入該日期查詢
    tr.addEventListener('click', () => {
      switchToDailyDate(entry.date);
    });

    tr.innerHTML = `
      <td style="font-weight: 600; color: var(--accent-cyan);">${escapeHtml(entry.date)}</td>
      <td style="color: var(--text-secondary);">${formatToken(entry.total_input_tokens || 0)}</td>
      <td style="color: var(--text-secondary);">${formatToken(entry.total_output_tokens || 0)}</td>
      <td style="color: #aeb9c8;">${formatToken(entry.total_reasoning_tokens || 0)}</td>
      <td style="color: #34d399;">${formatToken(entry.total_cache_read_tokens || 0)}</td>
      <td style="font-weight: 700; color: #fbbf24;">${formatToken(entry.total_tokens)}</td>
      <td style="font-weight: 700; color: var(--neon-gold);">${formatCost(entry.cost_usd || 0)}</td>
    `;
    tbody.appendChild(tr);
  });
}

function sortAndRenderMonthlyDailyTable() {
  if (!currentMonthlyBreakdown || currentMonthlyBreakdown.length === 0) {
    renderMonthlyDailySummaryTable([]);
    return;
  }

  currentMonthlyBreakdown.sort((a, b) => {
    let valA, valB;
    if (monthlyDailySortColumn === 'date') {
      valA = a.date;
      valB = b.date;
    } else {
      const keyMap = {
        'input': 'total_input_tokens',
        'output': 'total_output_tokens',
        'reasoning': 'total_reasoning_tokens',
        'cache': 'total_cache_read_tokens',
        'total': 'total_tokens',
        'cost': 'cost_usd'
      };
      const field = keyMap[monthlyDailySortColumn] || monthlyDailySortColumn;
      valA = a[field];
      valB = b[field];
    }

    // 空值處理
    if (valA === undefined || valA === null) valA = 0;
    if (valB === undefined || valB === null) valB = 0;

    if (typeof valA === 'string' && typeof valB === 'string') {
      return monthlyDailySortDirection === 'asc' 
        ? valA.localeCompare(valB) 
        : valB.localeCompare(valA);
    }

    return monthlyDailySortDirection === 'asc' ? valA - valB : valB - valA;
  });

  renderMonthlyDailySummaryTable(currentMonthlyBreakdown);
  updateMonthlySortHeadersUI();
}

function updateMonthlySortHeadersUI() {
  const headers = document.querySelectorAll('.premium-table th.sortable[data-table="monthly"]');
  headers.forEach(th => {
    const column = th.getAttribute('data-sort');
    const icon = th.querySelector('.sort-icon');
    if (!icon) return;

    th.classList.remove('sorted-asc', 'sorted-desc');
    
    if (column === monthlyDailySortColumn) {
      if (monthlyDailySortDirection === 'asc') {
        th.classList.add('sorted-asc');
        icon.innerHTML = iconMarkup('chevron-up', 'sort-glyph');
      } else {
        th.classList.add('sorted-desc');
        icon.innerHTML = iconMarkup('chevron-down', 'sort-glyph');
      }
    } else {
      icon.innerHTML = `<span class="sort-icon-placeholder">${iconMarkup('chevron-up', 'sort-glyph')}${iconMarkup('chevron-down', 'sort-glyph')}</span>`;
    }
  });
}

// =========================================================================
// 顯示精緻浮動通知 (Toast)
// =========================================================================
function showNotification(message, type = 'info') {
  console.log(`[${type.toUpperCase()}] ${message}`);
  
  let container = document.getElementById('toast-container');
  if (!container) {
    container = document.createElement('div');
    container.id = 'toast-container';
    container.style.position = 'fixed';
    container.style.bottom = '24px';
    container.style.right = '24px';
    container.style.zIndex = '9999';
    container.style.display = 'flex';
    container.style.flexDirection = 'column';
    container.style.gap = '10px';
    document.body.appendChild(container);
  }

  const toast = document.createElement('div');
  toast.className = 'glass-card';
  toast.style.padding = '12px 20px';
  toast.style.borderRadius = '10px';
  toast.style.boxShadow = 'var(--shadow-lg)';
  toast.style.border = '1px solid var(--glass-border)';
  toast.style.animation = 'slideIn 0.3s cubic-bezier(0.4, 0, 0.2, 1)';
  toast.style.display = 'flex';
  toast.style.alignItems = 'center';
  toast.style.gap = '10px';
  toast.style.fontSize = '13px';
  toast.style.fontWeight = '500';

  if (!document.getElementById('toast-animation-styles')) {
    const style = document.createElement('style');
    style.id = 'toast-animation-styles';
    style.innerHTML = `
      @keyframes slideIn {
        from { opacity: 0; transform: translateY(20px); }
        to { opacity: 1; transform: translateY(0); }
      }
      @keyframes fadeOut {
        from { opacity: 1; transform: translateY(0); }
        to { opacity: 0; transform: translateY(-20px); }
      }
    `;
    document.head.appendChild(style);
  }

  let icon = 'INFO';
  let color = 'var(--accent-cyan)';
  if (type === 'success') {
    icon = 'OK';
    color = 'var(--neon-green)';
  } else if (type === 'error') {
    icon = 'ERR';
    color = 'var(--neon-red)';
  }

  toast.innerHTML = `<span class="toast-kind" style="color: ${color};">${icon}</span> <span style="color: ${color}; font-family: var(--font-display);">${message}</span>`;
  container.appendChild(toast);

  setTimeout(() => {
    toast.style.animation = 'fadeOut 0.3s cubic-bezier(0.4, 0, 0.2, 1)';
    toast.addEventListener('animationend', () => {
      toast.remove();
    });
  }, 3000);
}

// =========================================================================
// 主題切換 (Light / Dark Theme Toggle)
// =========================================================================
function initThemeToggle() {
  const savedTheme = localStorage.getItem('theme') || 'dark';
  document.documentElement.setAttribute('data-theme', savedTheme);
  updateThemeButton(savedTheme);

  const themeBtn = document.getElementById('theme-toggle-btn');
  if (themeBtn) {
    themeBtn.addEventListener('click', () => {
      const currentTheme = document.documentElement.getAttribute('data-theme') || 'dark';
      const newTheme = currentTheme === 'dark' ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', newTheme);
      localStorage.setItem('theme', newTheme);
      updateThemeButton(newTheme);
      
      // 動態更新 Chart.js 顏色
      updateChartsTheme(newTheme);
    });
  }
}

function updateThemeButton(theme) {
  const themeBtn = document.getElementById('theme-toggle-btn');
  if (themeBtn) {
    const title = theme === 'dark' ? t('theme_toggle_title_dark') : t('theme_toggle_title_light');
    themeBtn.innerHTML = iconMarkup('theme');
    themeBtn.title = title;
    themeBtn.setAttribute('aria-label', title);
  }
}

function updateChartsTheme(theme) {
  const isLight = theme === 'light';
  const textColor = isLight ? '#1e293b' : '#f3f4f6';
  const mutedColor = isLight ? '#64748b' : '#9ca3af';
  const gridColor = isLight ? 'rgba(15, 23, 42, 0.05)' : 'rgba(255, 255, 255, 0.05)';
  const tooltipBg = isLight ? 'rgba(255, 255, 255, 0.95)' : 'rgba(15, 18, 29, 0.95)';
  const tooltipBorder = isLight ? 'rgba(15, 23, 42, 0.1)' : 'rgba(255, 255, 255, 0.1)';

  [tokenChartInstance, monthlyChartInstance, yearlyChartInstance].forEach(chart => {
    if (chart) {
      // 更新標籤文字顏色
      if (chart.options.plugins.legend && chart.options.plugins.legend.labels) {
        chart.options.plugins.legend.labels.color = textColor;
      }
      // 更新 Tooltip 樣式
      if (chart.options.plugins.tooltip) {
        chart.options.plugins.tooltip.backgroundColor = tooltipBg;
        chart.options.plugins.tooltip.titleColor = chartPalette.tokenStroke;
        chart.options.plugins.tooltip.bodyColor = textColor;
        chart.options.plugins.tooltip.borderColor = tooltipBorder;
      }
      // 更新軸線刻度與網格顏色
      if (chart.options.scales) {
        Object.keys(chart.options.scales).forEach(scaleKey => {
          const scale = chart.options.scales[scaleKey];
          if (scale.grid) {
            scale.grid.color = gridColor;
          }
          if (scale.ticks) {
            scale.ticks.color = mutedColor;
          }
          if (scale.title) {
            scale.title.color = textColor;
          }
        });
      }
      chart.update();
    }
  });
}

// =========================================================================
// Setup Guide Modal & Clipboard Dynamic Logic
// =========================================================================
function initSetupGuide() {
  const setupBtn = document.getElementById('btn-setup-guide');
  const closeBtn = document.getElementById('close-setup-modal-btn');
  const modalOverlay = document.getElementById('setup-guide-modal');

  if (setupBtn && modalOverlay) {
    setupBtn.addEventListener('click', openSetupModal);
  }

  if (closeBtn && modalOverlay) {
    closeBtn.addEventListener('click', closeSetupModal);
    modalOverlay.addEventListener('click', (e) => {
      if (e.target === modalOverlay) {
        closeSetupModal();
      }
    });
  }

  // Bind Escape key to close setup modal
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeSetupModal();
    }
  });

  // Load absolute script path info and build clipboard configs dynamically
  loadSetupInfo();

  // Bind clipboard copy buttons
  initClipboardButtons();
}

function openSetupModal() {
  const modal = document.getElementById('setup-guide-modal');
  if (modal) {
    const statuslineBody = document.getElementById('setup-body-statusline');
    const codexBody = document.getElementById('setup-body-codex');
    const claudeBody = document.getElementById('setup-body-claude');
    if (statuslineBody) statuslineBody.style.display = 'none';
    if (codexBody) codexBody.style.display = 'none';
    if (claudeBody) claudeBody.style.display = 'none';

    if (currentAssistant === 'codex') {
      if (codexBody) codexBody.style.display = 'block';
    } else if (currentAssistant === 'claude') {
      if (claudeBody) claudeBody.style.display = 'block';
    } else {
      if (statuslineBody) statuslineBody.style.display = 'none';
      if (statuslineBody) statuslineBody.style.display = 'block';
    }
    loadSetupInfo();
    modal.classList.add('active');
  }
}

function closeSetupModal() {
  const modal = document.getElementById('setup-guide-modal');
  if (modal) {
    modal.classList.remove('active');
  }
}

async function loadSetupInfo() {
  try {
    const resolvedAssistant = isSupportedAssistant(currentAssistant) ? currentAssistant : 'antigravity';
    const res = await fetch(`/api/${resolvedAssistant}/setup-info`);
    const data = await res.json();
    
    // Dynamic values based on home_dir
    const homeDir = data.home_dir || '/home/user';

    // Localize modal title based on selected assistant
    const titleH2 = document.getElementById('setup-modal-title');
    if (titleH2) {
      if (currentAssistant === 'copilot') {
        titleH2.setAttribute('data-i18n', 'copilot_setup_modal_title');
      } else if (currentAssistant === 'codex') {
        titleH2.setAttribute('data-i18n', 'codex_setup_modal_title');
      } else if (currentAssistant === 'claude') {
        titleH2.setAttribute('data-i18n', 'claude_setup_modal_title');
      } else {
        titleH2.setAttribute('data-i18n', 'setup_modal_title');
      }
    }
    
    if (currentAssistant === 'antigravity' || currentAssistant === 'copilot') {
      const folder = currentAssistant === 'copilot' ? '.copilot' : '.gemini/antigravity-cli';
      const targetScriptPath = `$HOME/${folder}/statusline-token.sh`;

      const settingsJson = JSON.stringify({
        "statusLine": {
          "type": "command",
          "command": targetScriptPath,
          "padding": 1
        }
      }, null, 2);

      const mergedJson = JSON.stringify({
        "footer": {
          "showDirectory": true,
          "showBranch": true
        },
        "statusLine": {
          "type": "command",
          "command": targetScriptPath,
          "padding": 1
        }
      }, null, 2);

      // Localize step instructions dynamically
      const introP = document.getElementById('setup-modal-intro');
      const stepCloneH3 = document.getElementById('setup-step-clone');
      const stepCloneDescP = document.getElementById('setup-step-clone-desc');
      const step1H3 = document.getElementById('setup-step-1');
      const step1DescP = document.getElementById('setup-step-1-desc');
      const step2H3 = document.getElementById('setup-step-2');
      const step2DescP = document.getElementById('setup-step-2-desc');
      const step3H3 = document.getElementById('setup-step-3');
      const step4H3 = document.getElementById('setup-step-4');
      const step4DescP = document.getElementById('setup-step-4-desc');
      const step5H3 = document.getElementById('setup-step-5');
      const step5DescP = document.getElementById('setup-step-5-desc');

      if (currentAssistant === 'copilot') {
        if (introP) introP.setAttribute('data-i18n', 'copilot_setup_modal_intro');
        if (stepCloneH3) stepCloneH3.setAttribute('data-i18n', 'copilot_setup_step_clone');
        if (stepCloneDescP) stepCloneDescP.setAttribute('data-i18n', 'copilot_setup_step_clone_desc');
        if (step1H3) step1H3.setAttribute('data-i18n', 'copilot_setup_step_1');
        if (step1DescP) step1DescP.setAttribute('data-i18n', 'copilot_setup_step_1_desc');
        if (step2H3) step2H3.setAttribute('data-i18n', 'copilot_setup_step_2');
        if (step2DescP) step2DescP.setAttribute('data-i18n', 'copilot_setup_step_2_desc');
        if (step3H3) step3H3.setAttribute('data-i18n', 'copilot_setup_step_3');
        if (step4H3) step4H3.setAttribute('data-i18n', 'copilot_setup_step_4');
        if (step4DescP) step4DescP.setAttribute('data-i18n', 'copilot_setup_step_4_desc');
        if (step5H3) step5H3.setAttribute('data-i18n', 'copilot_setup_step_5');
        if (step5DescP) step5DescP.setAttribute('data-i18n', 'copilot_setup_step_5_desc');
      } else {
        if (introP) introP.setAttribute('data-i18n', 'setup_modal_intro');
        if (stepCloneH3) stepCloneH3.setAttribute('data-i18n', 'setup_step_clone');
        if (stepCloneDescP) stepCloneDescP.setAttribute('data-i18n', 'setup_step_clone_desc');
        if (step1H3) step1H3.setAttribute('data-i18n', 'setup_step_1');
        if (step1DescP) step1DescP.setAttribute('data-i18n', 'setup_step_1_desc');
        if (step2H3) step2H3.setAttribute('data-i18n', 'setup_step_2');
        if (step2DescP) step2DescP.setAttribute('data-i18n', 'setup_step_2_desc');
        if (step3H3) step3H3.setAttribute('data-i18n', 'setup_step_3');
        if (step4H3) step4H3.setAttribute('data-i18n', 'setup_step_4');
        if (step4DescP) step4DescP.setAttribute('data-i18n', 'setup_step_4_desc');
        if (step5H3) step5H3.setAttribute('data-i18n', 'setup_step_5');
        if (step5DescP) step5DescP.setAttribute('data-i18n', 'setup_step_5_desc');
      }

      // Render to DOM
      const jsonCodeEl = document.getElementById('code-setup-json');
      const mergeCodeEl = document.getElementById('code-setup-json-merge');
      const setupCmdEl = document.getElementById('code-setup-cmd');
      const troubleshootAEl = document.getElementById('code-troubleshoot-a');
      const troubleshootBEl = document.getElementById('code-troubleshoot-b');

      const copyJsonBtn = document.getElementById('btn-copy-json');
      const copyMergeBtn = document.getElementById('btn-copy-json-merge');

      if (jsonCodeEl) jsonCodeEl.textContent = settingsJson;
      if (copyJsonBtn) copyJsonBtn.setAttribute('data-clipboard-text', settingsJson);
      
      if (mergeCodeEl) mergeCodeEl.textContent = mergedJson;
      if (copyMergeBtn) copyMergeBtn.setAttribute('data-clipboard-text', mergedJson);

      const editCmdEl = document.getElementById('code-edit-settings');
      if (editCmdEl) {
        editCmdEl.textContent = `vi ~/${folder}/settings.json`;
      }

      if (setupCmdEl) {
        const srcScript = `shell/${currentAssistant === 'copilot' ? 'copilot' : 'antigravity'}/statusline-token.sh`;
        setupCmdEl.textContent = `mkdir -p ~/${folder} && cp ${srcScript} ~/${folder}/statusline-token.sh && chmod +x ~/${folder}/statusline-token.sh`;
      }
      if (troubleshootAEl) {
        troubleshootAEl.textContent = `echo '{}' | ~/${folder}/statusline-token.sh`;
      }
      if (troubleshootBEl) {
        const displaySettingsPath = currentAssistant === 'copilot' 
          ? `~/.copilot/settings.json`
          : `~/.gemini/antigravity-cli/settings.json`;
        troubleshootBEl.textContent = `jq . ${displaySettingsPath}`;
      }
    } else if (currentAssistant === 'codex') {
      const homeLabelCodex = document.getElementById('lbl-detected-home-codex');
      if (homeLabelCodex) homeLabelCodex.textContent = `${homeDir}/.codex/sessions`;
    } else if (currentAssistant === 'claude') {
      const homeLabelClaude = document.getElementById('lbl-detected-home-claude');
      if (homeLabelClaude) homeLabelClaude.textContent = `${homeDir}/.claude/projects`;
    }

    // Apply updated language translations
    updateLanguageUI();

  } catch (err) {
    console.error('Failed to load dynamic setup paths:', err);
  }
}

function initClipboardButtons() {
  const copyButtons = document.querySelectorAll('.copy-code-btn');
  
  copyButtons.forEach((btn) => {
    btn.addEventListener('click', () => {
      // Prioritize data-clipboard-text, fallback to next code/pre element's textContent
      let textToCopy = btn.getAttribute('data-clipboard-text');
      if (!textToCopy) {
        const codeEl = btn.nextElementSibling.querySelector('code') || btn.nextElementSibling;
        textToCopy = codeEl ? codeEl.textContent : '';
      }
      
      navigator.clipboard.writeText(textToCopy.trim()).then(() => {
        const originalText = btn.textContent;
        btn.textContent = t('copy_success');
        btn.classList.add('copied');
        
        setTimeout(() => {
          btn.textContent = originalText;
          btn.classList.remove('copied');
        }, 2000);
      }).catch((err) => {
        console.error('Failed to copy text: ', err);
        showNotification(t('copy_failed'), 'error');
      });
    });
  });
}

function toggleEmptyState(showEmpty) {
  isEmptyState = showEmpty;
  const emptyContainer = document.getElementById('empty-state-container');
  const dailyView = document.getElementById('daily-view-container');
  const monthlyView = document.getElementById('monthly-view-container');
  const yearlyView = document.getElementById('yearly-view-container');
  
  if (showEmpty) {
    if (emptyContainer) {
      emptyContainer.classList.remove('hidden');
      if (currentAssistant === 'none') {
        emptyContainer.innerHTML = `
          <div class="welcome-setup-card no-agent-card">
            ${cardIconMarkup('alert')}
            <h2>${t('no_agent_selected_title')}</h2>
            <p>${t('no_agent_selected_desc')}</p>
          </div>
        `;
      } else {
        const meta = getAssistantMeta(currentAssistant);
        let emptyLogoUrl = meta.logo;
        emptyContainer.innerHTML = `
          <div class="welcome-setup-card">
            <div class="card-icon" style="display: flex; justify-content: center; align-items: center; filter: drop-shadow(0 0 10px rgba(255,255,255,0.1)); margin-bottom: 12px;">
              <img src="${emptyLogoUrl}" alt="${meta.alt}" style="width: 48px; height: 48px; border-radius: 8px; object-fit: cover;" />
            </div>
            <h2>${t('empty_title')}</h2>
            <p>${t('empty_desc')}</p>
            <div class="action-buttons">
              <button class="primary-btn" id="btn-empty-setup-guide">${t('btn_empty_setup')}</button>
              <button class="secondary-btn" id="btn-empty-refresh">${t('btn_empty_refresh')}</button>
            </div>
          </div>
        `;
        
        const emptyGuideBtn = document.getElementById('btn-empty-setup-guide');
        if (emptyGuideBtn) {
          emptyGuideBtn.addEventListener('click', openSetupModal);
        }
        
        const emptyRefreshBtn = document.getElementById('btn-empty-refresh');
        if (emptyRefreshBtn) {
          emptyRefreshBtn.addEventListener('click', async () => {
            emptyRefreshBtn.classList.add('loading');
            await fetchDates();
            emptyRefreshBtn.classList.remove('loading');
          });
        }
      }
    }
    
    if (dailyView) dailyView.classList.add('hidden');
    if (monthlyView) monthlyView.classList.add('hidden');
    if (yearlyView) yearlyView.classList.add('hidden');
  } else {
    if (emptyContainer) {
      emptyContainer.classList.add('hidden');
    }
    
    if (dailyView) dailyView.classList.add('hidden');
    if (monthlyView) monthlyView.classList.add('hidden');
    if (yearlyView) yearlyView.classList.add('hidden');

    if (activeTab === 'daily') {
      if (dailyView) dailyView.classList.remove('hidden');
    } else if (activeTab === 'monthly') {
      if (monthlyView) monthlyView.classList.remove('hidden');
    } else if (activeTab === 'yearly') {
      if (yearlyView) yearlyView.classList.remove('hidden');
    }
  }
  updateCodexRateLimit();
}

// 點擊月度彙整圖表跳轉到每日即時
function switchToDailyDate(date) {
  const dateSelect = document.getElementById('date-select');
  if (!dateSelect) return;

  dateSelect.value = date;

  // 切換 Tab 到 daily
  if (activeTab === 'daily') {
    loadUsageData(date);
  } else {
    // switchTab('daily') 內部會自動載入 dateSelect.value
    switchTab('daily');
  }
}

// =========================================================================
// Pricing Rules & Modal Logic
// =========================================================================
async function fetchPricingRules() {
  try {
    const res = await fetch(`/api/${currentAssistant}/pricing`);
    if (res.ok) {
      pricingRules = await res.json();
      console.log('Loaded pricing rules:', pricingRules);
    } else {
      console.error('Failed to fetch pricing rules');
    }
  } catch (err) {
    console.error('Error fetching pricing rules:', err);
  }
}

function initPricingModal() {
  const pricingBtn = document.getElementById('btn-pricing-sheet');
  const closeBtn = document.getElementById('close-pricing-modal-btn');
  const modalOverlay = document.getElementById('pricing-modal');

  if (pricingBtn && modalOverlay) {
    pricingBtn.addEventListener('click', openPricingModal);
  }

  if (closeBtn && modalOverlay) {
    closeBtn.addEventListener('click', closePricingModal);
    modalOverlay.addEventListener('click', (e) => {
      if (e.target === modalOverlay) {
        closePricingModal();
      }
    });
  }

  // Bind Escape key to close pricing modal
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closePricingModal();
    }
  });
}

function openPricingModal() {
  const modal = document.getElementById('pricing-modal');
  if (modal) {
    modal.classList.add('active');
    renderPricingModalTable();
  }
}

function closePricingModal() {
  const modal = document.getElementById('pricing-modal');
  if (modal) {
    modal.classList.remove('active');
  }
}

function renderPricingModalTable() {
  const tbody = document.getElementById('pricing-table-body');
  if (!tbody) return;
  tbody.innerHTML = '';

  if (!pricingRules || pricingRules.length === 0) {
    tbody.innerHTML = '<tr><td colspan="7" class="placeholder-text">載入中...</td></tr>';
    return;
  }

  pricingRules.forEach(r => {
    const tr = document.createElement('tr');
    tr.style.cursor = 'default';
    tr.innerHTML = `
      <td style="font-weight: 600;"><span class="badge highlight">${escapeHtml(r.model_name)}</span></td>
      <td>${escapeHtml(r.deployment_type)}</td>
      <td>${escapeHtml(r.unit)}</td>
      <td style="color: var(--accent-cyan); font-weight: 600;">$${r.input_price.toFixed(2)}</td>
      <td style="color: #34d399; font-weight: 600;">$${r.cache_input_price.toFixed(2)}</td>
      <td style="color: #aeb9c8; font-weight: 600;">$${r.output_price.toFixed(2)}</td>
      <td style="color: var(--text-secondary);">${escapeHtml(r.batch_api_price)}</td>
    `;
    tbody.appendChild(tr);
  });
}

function formatCost(cost) {
  if (cost === null || cost === undefined) return '-';
  const c = Number(cost);
  if (isNaN(c)) return '-';
  if (c < 1000) return '$' + c.toFixed(2);
  if (c < 10000) return '$' + c.toFixed(1);
  return '$' + c.toFixed(0);
}

async function updateCodexRateLimit() {
  const container = document.getElementById('codex-control-panel');
  if (container) container.classList.add('hidden');
}

async function updateCodexResets(forceRefresh = false) {
  return Promise.resolve(forceRefresh);
}

function renderCodexResets(cachedData) {
  return cachedData;
}

function formatDateTime(dateObj) {
  if (!dateObj || isNaN(dateObj.getTime())) return '';
  const pad = (num) => String(num).padStart(2, '0');
  const year = dateObj.getFullYear();
  const month = pad(dateObj.getMonth() + 1);
  const date = pad(dateObj.getDate());
  const hours = pad(dateObj.getHours());
  const minutes = pad(dateObj.getMinutes());
  const seconds = pad(dateObj.getSeconds());
  return `${year}-${month}-${date} ${hours}:${minutes}:${seconds}`;
}

async function updateCodexAuthSwitcher() {
  return Promise.resolve();
}
