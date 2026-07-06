---
version: alpha
name: Token Usage Insights
description: Local-first AI CLI token usage and session reconstruction dashboard.
colors:
  app-bg: "#07090D"
  sidebar-bg: "#0B0E14"
  drawer-bg: "#0D1118"
  card-bg: "#0D1118"
  card-bg-hover: "#111722"
  glass-border: "#DBE4EE1C"
  glass-border-focus: "#00D2E061"
  text-primary: "#F4F7FB"
  text-secondary: "#C8D2DF"
  text-muted: "#94A3B8"
  accent-cyan: "#00D2E0"
  accent-blue: "#2D8CFF"
  accent-purple: "#A78BFA"
  state-success: "#31D0AA"
  state-danger: "#FF5C8A"
  state-warning: "#F6BE4F"
  light-bg: "#F6F8FB"
  light-sidebar: "#FFFFFF"
  light-text-primary: "#172033"
  light-text-secondary: "#475569"
typography:
  display:
    fontFamily: "Inter, Outfit, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "1.35rem"
    fontWeight: 750
    lineHeight: 1.2
    letterSpacing: "0"
  title:
    fontFamily: "Inter, Outfit, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "0.96rem"
    fontWeight: 700
    lineHeight: 1.2
    letterSpacing: "0.3px"
  body:
    fontFamily: "Inter, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 500
    lineHeight: 1.6
    letterSpacing: "0"
  label:
    fontFamily: "Inter, Outfit, system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "0.5px"
  data:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace"
    fontSize: "13px"
    fontWeight: 600
    lineHeight: 1.4
    letterSpacing: "0"
rounded:
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "10px"
  xl: "12px"
  panel: "14px"
  pill: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "20px"
  panel: "24px"
  page: "32px"
components:
  button-primary:
    backgroundColor: "{colors.accent-cyan}"
    textColor: "{colors.app-bg}"
    typography: "{typography.label}"
    rounded: "{rounded.pill}"
    padding: "12px 28px"
  button-icon:
    backgroundColor: "{colors.card-bg}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.lg}"
    width: "42px"
    height: "42px"
  card-stat:
    backgroundColor: "{colors.card-bg}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.panel}"
    padding: "20px"
  input-select:
    backgroundColor: "{colors.card-bg}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.xl}"
    padding: "12px 16px"
---

# Design System: Token Usage Insights

## Overview

Creative North Star: "Local Operations Console"

Token Usage Insights 的視覺系統應像本機開發者工作台上的操作主控台：資料密集、層級清楚、反應直接。重構後的介面以更克制的深色 surface 階層、青色互動狀態、側欄控制、sticky header、圖表、表格與右側 session drawer 組成；未來設計要延續這種工具型結構，而不是轉向展示型或行銷型頁面。

整體策略是 restrained product UI。深色主題是目前主要工作環境，淺色主題用於高環境光或偏好設定，但兩者都必須保留同一套元件語彙。毛玻璃、光暈與漸層已被壓低為結構輔助，不能作為預設裝飾。

Key Characteristics:

- 密集但可掃描：側欄、卡片、圖表、表格與 drawer 都服務快速判讀。
- 本地可信：文案與狀態應清楚指出資料來自本機日誌與 SQLite。
- 操作優先：日期、月份、年份、agent、語言、主題與同步動作都要有一致控制樣式。
- 可追溯：總覽數字必須能回到 session、turn、工具呼叫與原始對話上下文。

## Colors

色彩以深色 neutral surface 為底，青色作為主要互動與資料焦點，紫色、綠色、紅色與金色只用於狀態或資料角色。

### Primary

- Deep Console Black (#07090D): App 背景，用於降低長時間監控的視覺疲勞。
- Operations Cyan (#00D2E0): 主要互動、focus ring、目前排序、active 狀態與關鍵 token 指標。
- Signal Blue (#2D8CFF): 與 cyan 組成極少量 primary action 漸層；只應出現在主要 action 或高亮資料上。

### Secondary

- Operational Purple (#A78BFA): 次要時間維度控制、agent reply、推理相關標記。
- Success Green (#31D0AA): 成功、複製完成、快取正向訊號。
- Danger Pink (#FF5C8A): 即時刷新警示、錯誤、危險或需注意狀態。
- Warning Gold (#F6BE4F): 費用、總 token、警告與高消耗提示。

### Neutral

- Sidebar Black (#0B0E14): 側欄與控制面板背景。
- Drawer Black (#0D1118): session drawer 與 modal 的高層級面板。
- Operations Surface (#0D1118): 卡片與圖表容器背景。
- Hairline Border (#DBE4EE1C): 深色主題下的邊界與分隔線。
- Primary Text (#F4F7FB): 主要文字。
- Secondary Text (#C8D2DF): 描述、次要標籤與一般輔助文字。
- Muted Text (#94A3B8): 欄位標籤、meta 與低優先資訊。

Named Rules:

The Accent Rarity Rule. Cyan 是主要互動色，不是裝飾色；同一畫面中應集中用於目前選取、主要行動、focus 與最關鍵的資料角色。

The Data Role Rule. 綠、紅、金、紫必須對應狀態或資料語意，不應只是為了讓畫面更熱鬧。

## Typography

Display Font: Inter, fallback to Outfit and system UI.
Body Font: Inter, fallback to system UI.
Label/Mono Font: ui-monospace stack for IDs, paths, token values and code-like metadata.

Character: Inter 負責大多數產品 UI 角色，Outfit 僅保留為既有 fallback。整體應維持產品介面的緊密比例，不使用大型流體 hero heading。

### Hierarchy

- Display (750, 1.35rem, 1.2): 頁首目前日期、月份、年份與主要 view title。
- Title (700, 0.96rem, 1.2): 側欄品牌、卡片標題、modal / drawer title。
- Body (500, 0.875rem, 1.6): 說明、modal 內容、timeline 文字與一般 UI copy。
- Label (600, 0.75rem, 0.04-0.06em letter spacing): 欄位標籤、表頭、control label、badge。
- Data (600, 13px, monospace): session id、工作路徑、成本、token 數與 code-like 內容。

Named Rules:

The Product Scale Rule. 不使用 viewport-fluid heading；資料工具中的標題大小應穩定，避免因 viewport 寬度改變而破壞掃描節奏。

The No Decorative Type Rule. 不使用漸層文字作為常態；若沿用既有漸層標題，應限制在單一頁首或 modal title，且不得降低可讀性。

## Elevation

目前系統使用 tonal layering、實色 surface、細邊界與極少量陰影的混合策略。深色主題的深度主要來自背景層次與邊界，陰影只在 drawer、modal 或 mobile sidebar 上輔助分層。

### Shadow Vocabulary

- Shadow Small (`0 1px 2px rgba(0, 0, 0, 0.24)`): 小型控制與按鈕的最低層級陰影。
- Shadow Large (`0 12px 28px rgba(0, 0, 0, 0.28)`): 高層級容器或浮層。
- Panel Edge (`-6px 0 12px rgba(0, 0, 0, 0.22)`): 右側 drawer 的方向性陰影。

Named Rules:

The Structural Blur Rule. `backdrop-filter` 只用於 sticky header、drawer、modal 或必要的 layered surface；不得作為整頁背景裝飾。

The One Lift Rule. 同一個元件不要同時使用強邊框、寬模糊陰影與光暈。若需要 emphasis，優先選擇狀態色、背景 tint 或邊界變化。

## Components

### Buttons

- Shape: 一般工具按鈕使用 8-12px；icon button 使用 10px，不做過圓卡片。
- Primary: cyan solid 或極少量 cyan-to-blue gradient，深色文字，padding 約 12px 28px，只用於主要 action。
- Hover / Focus: hover 可輕微 translate 或 scale，focus-visible 必須提供 cyan ring 或明確 border。
- Secondary / Ghost: 半透明背景加細邊界，避免額外大陰影。

### Chips

- Style: agent badge、token badge 與狀態 badge 使用低透明度 tint、1px border 與 4-8px radius。
- State: active 狀態必須比 hover 更明確，可用 cyan border、背景 tint 與字重提升。

### Cards / Containers

- Corner Style: 一般統計卡使用 14px；mini cards 使用 10px；drawer meta item 使用 10px。
- Background: `--card-bg` 或較低透明度 neutral surface。
- Shadow Strategy: 預設低陰影；高層級 modal / drawer 才使用 glass shadow。
- Border: 1px `--glass-border`，狀態高亮才改為語意色透明 border。
- Internal Padding: 統計卡 20px、圖表與大面板 24px、mini cards 10-12px。

### Inputs / Fields

- Style: select、date input 與 mini select 使用半透明背景、1px border、10-12px radius。
- Focus: border 轉 cyan，並使用 `--glass-border-focus` ring。
- Disabled: 降低 opacity，保留文字可讀性，不只改變游標。

### Navigation

側欄是主要控制區，desktop 使用 280px 固定欄；992px 以下改為可收合 fixed drawer。tab、agent dropdown、日期控制、即時刷新設定應共享同一套低透明背景、細邊界與 compact label 語彙。

### Session Timeline Drawer

右側 drawer 是產品的 signature component。它應以 session metadata、token stats、user / assistant bubble、工具呼叫與 Markdown reply 為核心，使用 timeline line、dot、bubble radius 與語意色區分角色。長內容必須可展開收合，不能破壞 drawer 寬度。

## Do's and Don'ts

### Do:

- Do keep dense dashboards readable with clear section spacing, 1px dividers, stable table columns and explicit empty states.
- Do use Operations Cyan (#00D2E0) for active selection, primary action and focus-visible states.
- Do preserve bilingual UI copy with short labels and avoid wrapping that breaks controls on mobile.
- Do include reduced-motion alternatives for auto-refresh, drawer, modal and timeline transitions.
- Do make long CWD paths, session IDs and model names truncate or wrap predictably.

### Don't:

- Don't turn the dashboard into a marketing landing page, generic SaaS hero or hero metric template.
- Don't use decorative glassmorphism, over-neon glow, purple-blue gradients or large shadows when they do not clarify state or hierarchy.
- Don't rely on color alone to communicate success, warning, error, active or selected states.
- Don't use low-contrast gray text on dark or tinted backgrounds.
- Don't use side-stripe borders, repeated identical card grids, decorative grid backgrounds or gradient text as a default pattern.
