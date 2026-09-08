const siteTranslations = {
  'zh-TW': {
    meta_locale: 'zh_TW', meta_title: 'Token 戰情室｜本機優先的 AI Coding Agent Token 看板',
    brand_name: 'Token 戰情室',
    meta_description: 'Token 戰情室集中分析多種 AI Coding Agent 的 Token、估算費用與完整 Session 時間軸。資料來自本機日誌與 SQLite。',
    skip: '跳到主要內容', brand_aria: 'Token 戰情室首頁', menu: '選單', language: '語言', language_auto: '自動',
    nav: '主要導覽', nav_sources: '支援來源', nav_workflow: '資料流', nav_features: '功能',
    install_now: '立即安裝', view_source: '查看原始碼', hero_title: '讀懂 Token 與 Session',
    hero_description: '從本機日誌統整用量、費用與完整 Session 時間軸。資料留在你的電腦。',
    hero_alt: 'Token 戰情室每日看板，顯示 Token 統計、估算費用、趨勢圖與 Session 清單',
    hero_caption: '每日看板，實際產品畫面', sources_title: '七種來源，一個視角',
    sources_aria: '支援的 AI Coding Agent 資料來源', workflow_title: '資料一路在本機',
    workflow_description: '不替你呼叫 AI 供應商 API。Token 戰情室讀取本機資料，整理進本機 SQLite，再交給瀏覽器看板。',
    workflow_logs_title: '本機日誌', workflow_logs_desc: '讀取各工具既有的 Session 記錄與 Status Line 收集檔。',
    workflow_db_title: '本機 SQLite', workflow_db_desc: '背景增量同步，集中整理 Session、模型、Token 與費用資料。',
    workflow_dashboard_title: '瀏覽器看板', workflow_dashboard_desc: '在 <code>localhost:3003</code> 檢視趨勢與工作脈絡。',
    workflow_alt: 'Token 戰情室瀏覽器看板縮圖', features_aria: '主要功能',
    analysis_alt: 'Token 戰情室趨勢分析與 Session 清單', analysis_title: '每日、月度、年度，一眼讀懂',
    analysis_desc: '拆解輸入、輸出、快取讀取、快取寫入與推理 Token，並依 <code>pricing.csv</code> 估算費用。',
    analysis_models: '模型分佈', analysis_models_desc: '看清 Token 用在哪些模型。',
    analysis_projects: '專案目錄', analysis_projects_desc: '依工作目錄追蹤投入。',
    analysis_sessions: 'Session 清單', analysis_sessions_desc: '排序、篩選，再下鑽細節。',
    session_title: '回到完整 Session 脈絡', session_desc: '從使用者提示詞、助理回覆、推理內容到工具呼叫，一段段還原。',
    session_tools: '工具步驟', session_tools_desc: '保留參數、退出碼、stdout 與 stderr。',
    session_replies: '助理回覆', session_replies_desc: '以 Markdown 清楚呈現完整內容。',
    session_agents: 'Agent 關係', session_agents_desc: '辨識 Codex subagent 與父 Session。',
    mobile_alt: 'Token 戰情室行動版深色看板', privacy_title: '資料不必離開你的電腦',
    privacy_desc: '核心資料來源是本機日誌、Status Line 收集檔與本機 SQLite。分析與費用估算都在你的電腦上完成。',
    install_title: '一行啟動，直接開啟', install_desc: '使用 npx 或安裝腳本取得目前平台的已編譯版本，不需要 Rust 或 Cargo。',
    install_npx_note: '已有 Node.js 18.18 以上版本時，建議直接使用 npx；不會建立全域命令。',
    platform_select: '選擇執行方式', copy_command: '複製指令', launch_after: '指令啟動後開啟',
    license: 'MIT License · Open source', platform_support: 'Windows 10/11、macOS、Linux 與 WSL', copyright: 'Copyright © 2026 Token 戰情室 | Made by',
    course_link: 'AI 寫程式省錢術：從 Token 焦慮到團隊成本治理',
    copy_success: '已複製安裝指令。', copy_button_done: '已複製',
    copy_denied: '瀏覽器未允許自動複製，請手動選取指令。'
  },
  'zh-CN': {
    meta_locale: 'zh_CN', meta_title: 'Token 战情室｜本地优先的 AI Coding Agent Token 看板',
    brand_name: 'Token 战情室',
    meta_description: 'Token 战情室集中分析多种 AI Coding Agent 的 Token、估算费用与完整 Session 时间轴。数据来自本地日志与 SQLite。',
    skip: '跳至主要内容', brand_aria: 'Token 战情室首页', menu: '菜单', language: '语言', language_auto: '自动',
    nav: '主要导航', nav_sources: '支持来源', nav_workflow: '数据流', nav_features: '功能',
    install_now: '立即安装', view_source: '查看源码', hero_title: '读懂 Token 与 Session',
    hero_description: '从本地日志汇总用量、费用与完整 Session 时间轴。数据留在你的电脑。',
    hero_alt: 'Token 战情室每日看板，显示 Token 统计、估算费用、趋势图与 Session 列表',
    hero_caption: '每日看板，实际产品画面', sources_title: '七种来源，一个视角',
    sources_aria: '支持的 AI Coding Agent 数据来源', workflow_title: '数据始终在本地',
    workflow_description: '不替你调用 AI 供应商 API。Token 战情室读取本地数据，整理到本地 SQLite，再交给浏览器看板。',
    workflow_logs_title: '本地日志', workflow_logs_desc: '读取各工具已有的 Session 记录与 Status Line 收集文件。',
    workflow_db_title: '本地 SQLite', workflow_db_desc: '后台增量同步，集中整理 Session、模型、Token 与费用数据。',
    workflow_dashboard_title: '浏览器看板', workflow_dashboard_desc: '在 <code>localhost:3003</code> 查看趋势与工作脉络。',
    workflow_alt: 'Token 战情室浏览器看板缩略图', features_aria: '主要功能',
    analysis_alt: 'Token 战情室趋势分析与 Session 列表', analysis_title: '每日、每月、每年，一眼看懂',
    analysis_desc: '拆解输入、输出、缓存读取、缓存写入与推理 Token，并根据 <code>pricing.csv</code> 估算费用。',
    analysis_models: '模型分布', analysis_models_desc: '看清 Token 用在哪些模型。',
    analysis_projects: '项目目录', analysis_projects_desc: '按工作目录追踪投入。',
    analysis_sessions: 'Session 列表', analysis_sessions_desc: '排序、筛选，再深入查看细节。',
    session_title: '回到完整 Session 脉络', session_desc: '从用户提示词、助手回复、推理内容到工具调用，逐段还原。',
    session_tools: '工具步骤', session_tools_desc: '保留参数、退出码、stdout 与 stderr。',
    session_replies: '助手回复', session_replies_desc: '使用 Markdown 清晰呈现完整内容。',
    session_agents: 'Agent 关系', session_agents_desc: '识别 Codex subagent 与父 Session。',
    mobile_alt: 'Token 战情室移动版深色看板', privacy_title: '数据无需离开你的电脑',
    privacy_desc: '核心数据来自本地日志、Status Line 收集文件与本地 SQLite。分析与费用估算都在你的电脑上完成。',
    install_title: '一行启动，立即开启', install_desc: '使用 npx 或安装脚本获取当前平台的编译版本，无需 Rust 或 Cargo。',
    install_npx_note: '已有 Node.js 18.18 或更新版本时，建议直接使用 npx；不会创建全局命令。',
    platform_select: '选择运行方式', copy_command: '复制命令', launch_after: '命令启动后打开',
    license: 'MIT License · Open source', platform_support: 'Windows 10/11、macOS、Linux 与 WSL', copyright: 'Copyright © 2026 Token 战情室 | Made by',
    course_link: 'AI 编程省钱术：从 Token 焦虑到团队成本治理',
    copy_success: '安装命令已复制。', copy_button_done: '已复制',
    copy_denied: '浏览器不允许自动复制，请手动选择命令。'
  },
  en: {
    meta_locale: 'en_US', meta_title: 'Token War Room | Local-first AI Coding Agent Token dashboard',
    brand_name: 'Token War Room',
    meta_description: 'Token War Room analyzes Token usage, estimated costs, and complete Session timelines from local logs and SQLite.',
    skip: 'Skip to main content', brand_aria: 'Token War Room home', menu: 'Menu', language: 'Language', language_auto: 'Auto',
    nav: 'Primary navigation', nav_sources: 'Sources', nav_workflow: 'Data flow', nav_features: 'Features',
    install_now: 'Install now', view_source: 'View source', hero_title: 'Understand Tokens and Sessions',
    hero_description: 'Bring usage, costs, and complete Session timelines together from local logs. Your data stays on your computer.',
    hero_alt: 'Token War Room daily dashboard showing Token statistics, estimated cost, trend chart, and Session list',
    hero_caption: 'Daily dashboard, from the real product', sources_title: 'Seven sources, one view',
    sources_aria: 'Supported AI Coding Agent sources', workflow_title: 'The data stays local',
    workflow_description: 'Token War Room never calls AI provider APIs for you. It reads local data, organizes it in local SQLite, and presents it in the browser dashboard.',
    workflow_logs_title: 'Local logs', workflow_logs_desc: 'Reads existing Session records and Status Line collection files from each tool.',
    workflow_db_title: 'Local SQLite', workflow_db_desc: 'Incrementally syncs and organizes Sessions, models, Tokens, and costs in the background.',
    workflow_dashboard_title: 'Browser dashboard', workflow_dashboard_desc: 'Explore trends and working context at <code>localhost:3003</code>.',
    workflow_alt: 'Token War Room browser dashboard thumbnail', features_aria: 'Key features',
    analysis_alt: 'Token War Room trend analysis and Session list', analysis_title: 'Daily, monthly, yearly—understand it at a glance',
    analysis_desc: 'Break down input, output, cache read, cache write, and reasoning Tokens, with costs estimated from <code>pricing.csv</code>.',
    analysis_models: 'Model distribution', analysis_models_desc: 'See which models consume your Tokens.',
    analysis_projects: 'Project directories', analysis_projects_desc: 'Track investment by working directory.',
    analysis_sessions: 'Session list', analysis_sessions_desc: 'Sort, filter, and drill into details.',
    session_title: 'Return to the full Session context', session_desc: 'Reconstruct each step from user prompts and assistant replies to reasoning and tool calls.',
    session_tools: 'Tool steps', session_tools_desc: 'Keep arguments, exit codes, stdout, and stderr.',
    session_replies: 'Assistant replies', session_replies_desc: 'Render the complete response clearly in Markdown.',
    session_agents: 'Agent relationships', session_agents_desc: 'Identify Codex subagents and parent Sessions.',
    mobile_alt: 'Token War Room dark mobile dashboard', privacy_title: 'Your data never has to leave your computer',
    privacy_desc: 'The core sources are local logs, Status Line collection files, and local SQLite. Analysis and cost estimates run on your computer.',
    install_title: 'Start in one line, then open', install_desc: 'Use npx or an installer to get the compiled build for your platform—no Rust or Cargo required.',
    install_npx_note: 'If you have Node.js 18.18 or newer, use npx directly; it does not create a global command.',
    platform_select: 'Choose how to run', copy_command: 'Copy command', launch_after: 'Open after the command starts',
    license: 'MIT License · Open source', platform_support: 'Windows 10/11, macOS, Linux, and WSL', copyright: 'Copyright © 2026 Token War Room | Made by',
    course_link: 'Save money with AI coding: from Token anxiety to team cost governance',
    copy_success: 'Installation command copied.', copy_button_done: 'Copied',
    copy_denied: 'The browser blocked automatic copying. Select the command manually.'
  },
  ja: {
    meta_locale: 'ja_JP', meta_title: 'Token 戦情室｜ローカル優先の AI Coding Agent Token ダッシュボード',
    brand_name: 'Token 戦情室',
    meta_description: 'ローカルログと SQLite から Token 使用量、推定費用、Session のタイムラインを分析します。',
    skip: 'メインコンテンツへスキップ', brand_aria: 'Token 戦情室ホーム', menu: 'メニュー', language: '言語', language_auto: '自動',
    nav: 'メインナビゲーション', nav_sources: '対応ソース', nav_workflow: 'データフロー', nav_features: '機能',
    install_now: '今すぐインストール', view_source: 'ソースを見る', hero_title: 'Token と Session を理解する',
    hero_description: 'ローカルログから使用量、費用、Session のタイムラインを整理します。データは PC に残ります。',
    hero_alt: 'Token 統計、推定費用、トレンド、Session 一覧を表示する Token 戦情室の日次ダッシュボード',
    hero_caption: '実際の製品画面による日次ダッシュボード', sources_title: '7 つのソースを 1 つの視点で',
    sources_aria: '対応する AI Coding Agent のデータソース', workflow_title: 'データはローカルに保存',
    workflow_description: 'AI プロバイダー API は呼び出しません。ローカルデータを SQLite に整理し、ブラウザーで表示します。',
    workflow_logs_title: 'ローカルログ', workflow_logs_desc: '各ツールの Session 記録と Status Line ファイルを読み込みます。',
    workflow_db_title: 'ローカル SQLite', workflow_db_desc: 'Session、モデル、Token、費用をバックグラウンドで増分同期します。',
    workflow_dashboard_title: 'ブラウザーダッシュボード', workflow_dashboard_desc: '<code>localhost:3003</code> でトレンドと作業 context を確認できます。',
    workflow_alt: 'Token 戦情室ブラウザーダッシュボードのサムネイル', features_aria: '主な機能',
    analysis_alt: 'Token 戦情室のトレンド分析と Session 一覧', analysis_title: '日次・月次・年次をひと目で把握',
    analysis_desc: '入力、出力、キャッシュ、推論 Token を分解し、<code>pricing.csv</code> から費用を推定します。',
    analysis_models: 'モデル分布', analysis_models_desc: 'どのモデルで Token を使ったか確認できます。',
    analysis_projects: 'プロジェクトディレクトリ', analysis_projects_desc: '作業ディレクトリ別に利用状況を追跡します。',
    analysis_sessions: 'Session 一覧', analysis_sessions_desc: '並べ替え、絞り込み、詳細へドリルダウンできます。',
    session_title: '完全な Session コンテキストへ', session_desc: 'ユーザープロンプト、返信、推論、ツール呼び出しを順に復元します。',
    session_tools: 'ツール手順', session_tools_desc: '引数、終了コード、stdout、stderr を保持します。',
    session_replies: 'アシスタント返信', session_replies_desc: '完全な内容を Markdown で表示します。',
    session_agents: 'Agent の関係', session_agents_desc: 'Codex subagent と親 Session を識別します。',
    mobile_alt: 'Token 戦情室のダークモバイルダッシュボード', privacy_title: 'データを PC の外へ出す必要はありません',
    privacy_desc: 'ローカルログ、Status Line、SQLite を使い、分析と費用推定は PC 上で完結します。',
    install_title: '1 行で起動して開く', install_desc: 'npx またはインストーラーでプラットフォーム用ビルドを取得できます。Rust と Cargo は不要です。',
    install_npx_note: 'Node.js 18.18 以降がある場合は npx を推奨します。グローバルコマンドは作成されません。',
    platform_select: '実行方法を選択', copy_command: 'コマンドをコピー', launch_after: 'コマンド起動後に開く',
    license: 'MIT License · Open source', platform_support: 'Windows 10/11、macOS、Linux、WSL', copyright: 'Copyright © 2026 Token 戦情室 | Made by',
    course_link: 'AI コーディング節約術：Token 不安からチームのコスト管理へ',
    copy_success: 'インストールコマンドをコピーしました。', copy_button_done: 'コピー済み',
    copy_denied: '自動コピーが許可されませんでした。コマンドを手動で選択してください。'
  },
  ko: {
    meta_locale: 'ko_KR', meta_title: 'Token 전황실｜로컬 우선 AI Coding Agent Token 대시보드',
    brand_name: 'Token 전황실',
    meta_description: '로컬 로그와 SQLite에서 Token 사용량, 예상 비용, 완전한 Session 타임라인을 분석합니다.',
    skip: '본문으로 건너뛰기', brand_aria: 'Token 전황실 홈', menu: '메뉴', language: '언어', language_auto: '자동',
    nav: '주요 탐색', nav_sources: '지원 소스', nav_workflow: '데이터 흐름', nav_features: '기능',
    install_now: '지금 설치', view_source: '소스 보기', hero_title: 'Token과 Session을 이해하세요',
    hero_description: '로컬 로그에서 사용량, 비용, 완전한 Session 타임라인을 정리합니다. 데이터는 컴퓨터에 남습니다.',
    hero_alt: 'Token 통계, 예상 비용, 추이 차트, Session 목록을 보여 주는 Token 전황실 일일 대시보드',
    hero_caption: '실제 제품 화면의 일일 대시보드', sources_title: '7개 소스를 하나의 시각으로',
    sources_aria: '지원되는 AI Coding Agent 데이터 소스', workflow_title: '데이터는 로컬에 보관됩니다',
    workflow_description: 'AI 공급자 API를 대신 호출하지 않습니다. 로컬 데이터를 SQLite에 정리한 뒤 브라우저 대시보드로 보여 줍니다.',
    workflow_logs_title: '로컬 로그', workflow_logs_desc: '각 도구의 기존 Session 기록과 Status Line 수집 파일을 읽습니다.',
    workflow_db_title: '로컬 SQLite', workflow_db_desc: 'Session, 모델, Token, 비용을 백그라운드에서 증분 동기화합니다.',
    workflow_dashboard_title: '브라우저 대시보드', workflow_dashboard_desc: '<code>localhost:3003</code>에서 추이와 작업 맥락을 확인하세요.',
    workflow_alt: 'Token 전황실 브라우저 대시보드 미리보기', features_aria: '주요 기능',
    analysis_alt: 'Token 전황실 추이 분석과 Session 목록', analysis_title: '일간·월간·연간 데이터를 한눈에',
    analysis_desc: '입력, 출력, 캐시 읽기·쓰기, 추론 Token을 나누고 <code>pricing.csv</code>로 비용을 추정합니다.',
    analysis_models: '모델 분포', analysis_models_desc: 'Token을 사용한 모델을 확인하세요.',
    analysis_projects: '프로젝트 디렉터리', analysis_projects_desc: '작업 디렉터리별 사용량을 추적합니다.',
    analysis_sessions: 'Session 목록', analysis_sessions_desc: '정렬하고 필터링한 뒤 세부 내용을 확인하세요.',
    session_title: '완전한 Session 맥락으로 돌아가기', session_desc: '사용자 프롬프트, 답변, 추론, 도구 호출을 단계별로 복원합니다.',
    session_tools: '도구 단계', session_tools_desc: '인수, 종료 코드, stdout, stderr를 보존합니다.',
    session_replies: '어시스턴트 답변', session_replies_desc: '전체 내용을 Markdown으로 명확하게 표시합니다.',
    session_agents: 'Agent 관계', session_agents_desc: 'Codex subagent와 부모 Session을 식별합니다.',
    mobile_alt: 'Token 전황실 다크 모바일 대시보드', privacy_title: '데이터가 컴퓨터 밖으로 나갈 필요가 없습니다',
    privacy_desc: '로컬 로그, Status Line 수집 파일, 로컬 SQLite를 사용하며 분석과 비용 추정은 컴퓨터에서 수행됩니다.',
    install_title: '한 줄로 시작하고 바로 열기', install_desc: 'npx 또는 설치 프로그램으로 플랫폼용 빌드를 받을 수 있습니다. Rust와 Cargo는 필요하지 않습니다.',
    install_npx_note: 'Node.js 18.18 이상이 있다면 npx 사용을 권장합니다. 전역 명령은 생성되지 않습니다.',
    platform_select: '실행 방법 선택', copy_command: '명령 복사', launch_after: '명령 시작 후 열기',
    license: 'MIT License · Open source', platform_support: 'Windows 10/11, macOS, Linux, WSL', copyright: 'Copyright © 2026 Token 전황실 | Made by',
    course_link: 'AI 코딩 비용 절약: Token 불안에서 팀 비용 관리까지',
    copy_success: '설치 명령을 복사했습니다.', copy_button_done: '복사됨',
    copy_denied: '브라우저가 자동 복사를 허용하지 않았습니다. 명령을 직접 선택하세요.'
  }
};

const localeOrder = ['zh-TW', 'zh-CN', 'en', 'ja', 'ko'];
const localeOptions = ['auto', ...localeOrder];
const detectBrowserLocale = () => {
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language || ''];
  for (const language of languages) {
    const value = String(language).toLowerCase();
    if (value === 'zh-cn' || value === 'zh-sg' || value.startsWith('zh-cn-') || value.startsWith('zh-sg-')) return 'zh-CN';
    if (value === 'zh-tw' || value === 'zh-hk' || value === 'zh-mo' || value.startsWith('zh-tw-') || value.startsWith('zh-hk-') || value.startsWith('zh-mo-')) return 'zh-TW';
    if (value === 'en' || value.startsWith('en-')) return 'en';
    if (value === 'ja' || value.startsWith('ja-')) return 'ja';
    if (value === 'ko' || value.startsWith('ko-')) return 'ko';
  }
  return 'zh-TW';
};

const savedLocale = localStorage.getItem('site-lang');
let localePreference = localeOptions.includes(savedLocale) ? savedLocale : 'auto';
let currentLocale = localePreference === 'auto' ? detectBrowserLocale() : localePreference;
const t = (key) => siteTranslations[currentLocale][key] ?? siteTranslations.en[key] ?? key;

function applyLocale() {
  document.documentElement.lang = currentLocale;
  document.title = t('meta_title');
  document.querySelectorAll('[data-i18n]').forEach((element) => {
    element.innerHTML = t(element.dataset.i18n);
  });
  document.querySelectorAll('[data-i18n-aria-label]').forEach((element) => {
    element.setAttribute('aria-label', t(element.dataset.i18nAriaLabel));
  });
  document.querySelectorAll('[data-i18n-alt]').forEach((element) => {
    element.alt = t(element.dataset.i18nAlt);
  });
  document.querySelectorAll('[data-i18n-content]').forEach((element) => {
    element.setAttribute('content', t(element.dataset.i18nContent));
  });
  const structuredData = document.querySelector('script[type="application/ld+json"]');
  if (structuredData) {
    try {
      const json = JSON.parse(structuredData.textContent);
      json.name = t('brand_name');
      json.description = t('meta_description');
      json.inLanguage = currentLocale;
      structuredData.textContent = JSON.stringify(json);
    } catch {
      // Keep the original structured data if a host modifies the JSON block.
    }
  }
  const select = document.querySelector('#site-language-select');
  if (select) {
    select.value = localePreference;
  }
}

const menuToggle = document.querySelector('.menu-toggle');
const siteNav = document.querySelector('.site-nav');
if (menuToggle && siteNav) {
  const closeMenu = () => {
    menuToggle.setAttribute('aria-expanded', 'false');
    siteNav.classList.remove('is-open');
  };
  menuToggle.addEventListener('click', () => {
    const willOpen = menuToggle.getAttribute('aria-expanded') !== 'true';
    menuToggle.setAttribute('aria-expanded', String(willOpen));
    siteNav.classList.toggle('is-open', willOpen);
  });
  siteNav.addEventListener('click', (event) => {
    if (event.target instanceof HTMLAnchorElement) closeMenu();
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      closeMenu();
      menuToggle.focus();
    }
  });
}

const languageSelect = document.querySelector('#site-language-select');
languageSelect?.addEventListener('change', () => {
  localePreference = localeOptions.includes(languageSelect.value) ? languageSelect.value : 'auto';
  currentLocale = localePreference === 'auto' ? detectBrowserLocale() : localePreference;
  localStorage.setItem('site-lang', localePreference);
  applyLocale();
  const status = document.querySelector('.copy-status');
  if (status) status.textContent = '';
});

const tabs = Array.from(document.querySelectorAll('[data-command-tab]'));
const panels = Array.from(document.querySelectorAll('[data-command-panel]'));
const activateTab = (selectedTab) => {
  const selectedName = selectedTab.dataset.commandTab;
  tabs.forEach((tab) => {
    const isSelected = tab === selectedTab;
    tab.classList.toggle('is-active', isSelected);
    tab.setAttribute('aria-selected', String(isSelected));
    tab.tabIndex = isSelected ? 0 : -1;
  });
  panels.forEach((panel) => { panel.hidden = panel.dataset.commandPanel !== selectedName; });
};
tabs.forEach((tab, index) => {
  tab.addEventListener('click', () => activateTab(tab));
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    let nextIndex = index;
    if (event.key === 'ArrowLeft') nextIndex = (index - 1 + tabs.length) % tabs.length;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % tabs.length;
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = tabs.length - 1;
    activateTab(tabs[nextIndex]);
    tabs[nextIndex].focus();
  });
});

const copyStatus = document.querySelector('.copy-status');
document.querySelectorAll('[data-copy-command]').forEach((button) => {
  button.addEventListener('click', async () => {
    const panelName = button.dataset.copyCommand;
    const command = document.querySelector(`[data-command-panel="${panelName}"] [data-command]`)?.textContent?.trim();
    if (!command) return;
    try {
      await navigator.clipboard.writeText(command);
      if (copyStatus) copyStatus.textContent = t('copy_success');
      button.textContent = t('copy_button_done');
      window.setTimeout(() => { button.textContent = t('copy_command'); }, 1800);
    } catch {
      if (copyStatus) copyStatus.textContent = t('copy_denied');
    }
  });
});

applyLocale();
