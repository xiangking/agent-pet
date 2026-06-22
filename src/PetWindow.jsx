import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ATLAS_HEIGHT, ATLAS_WIDTH } from './constants'

const PANEL_POSITION_STORAGE_KEY = 'agent-pet-panel-positions-v4'
const PANEL_SIZE_STORAGE_KEY = 'agent-pet-panel-sizes-v1'
const MAX_VISIBLE_USAGE_METRICS = 24
const NOTICE_CARD_HEIGHT = 124
const NOTICE_TITLE_HEIGHT = 23
const NOTICE_OVERFLOW_HEIGHT = 23
const NOTICE_STACK_GAP = 8
const NOTICE_TOP_GAP = 12
const NOTICE_PANEL_MIN_HEIGHT = NOTICE_TITLE_HEIGHT + NOTICE_STACK_GAP + 88
const NOTICE_PANEL_HEIGHT = NOTICE_TITLE_HEIGHT + NOTICE_STACK_GAP + NOTICE_CARD_HEIGHT * 2 + NOTICE_STACK_GAP * 2

const PANEL_DEFAULT_SIZES = {
  usage: { width: 304, height: 258 },
  notices: { width: 292, height: NOTICE_PANEL_HEIGHT },
}

const PANEL_SIZE_LIMITS = {
  usage: { minWidth: 252, minHeight: 178, maxWidth: 560, maxHeight: 440 },
}

const PANEL_RESIZE_EDGES = ['left', 'right', 'top', 'bottom']

const CONTEXT_MENU_SIZE = {
  width: 34,
  height: 68,
}

const clampNumber = (value, min, max) => Math.min(Math.max(value, min), max)

const readStoredPanelPositions = () => {
  try {
    return JSON.parse(window.localStorage.getItem(PANEL_POSITION_STORAGE_KEY) || '{}')
  } catch {
    return {}
  }
}

const readStoredPanelSizes = () => {
  try {
    return JSON.parse(window.localStorage.getItem(PANEL_SIZE_STORAGE_KEY) || '{}')
  } catch {
    return {}
  }
}

const noticeIcon = {
  error: '!',
  warning: '!',
  success: '✓',
  info: 'i',
}

const noticeTypeLabel = (type, locale) => {
  const labels = {
    approval_required: { en: 'Approval', 'zh-CN': '需批准' },
    confirm_required: { en: 'Confirm', 'zh-CN': '需确认' },
    press_enter_required: { en: 'Continue', 'zh-CN': '需继续' },
    context_compacting: { en: 'Context', 'zh-CN': '上下文' },
    task_failed: { en: 'Failed', 'zh-CN': '失败' },
    info: { en: 'Notice', 'zh-CN': '提醒' },
  }
  return labels[type]?.[locale] || labels.info[locale] || labels.info.en
}

const noticeTranslationKeySuffix = (type) => {
  const suffixes = {
    approval_required: 'ApprovalRequired',
    confirm_required: 'ConfirmRequired',
    press_enter_required: 'PressEnterRequired',
    context_compacting: 'ContextCompacting',
    task_failed: 'TaskFailed',
    info: 'Info',
  }
  return suffixes[type] || suffixes.info
}

const formatWindowLabel = (minutes, locale) => {
  const value = Number(minutes)
  if (!Number.isFinite(value) || value <= 0) return ''
  if (value >= 60 * 24 * 7 && value % (60 * 24 * 7) === 0) {
    const weeks = value / (60 * 24 * 7)
    return locale === 'zh-CN' ? `${weeks} 周窗口` : `${weeks}w window`
  }
  if (value >= 60 * 24 && value % (60 * 24) === 0) {
    const days = value / (60 * 24)
    return locale === 'zh-CN' ? `${days} 天窗口` : `${days}d window`
  }
  if (value >= 60 && value % 60 === 0) {
    const hours = value / 60
    return locale === 'zh-CN' ? `${hours} 小时窗口` : `${hours}h window`
  }
  return locale === 'zh-CN' ? `${value} 分钟窗口` : `${value}m window`
}

const formatResetLabel = (resetsAt, locale) => {
  const resetSeconds = Number(resetsAt)
  if (!Number.isFinite(resetSeconds) || resetSeconds <= 0) return ''
  const diffMinutes = Math.max(0, Math.ceil((resetSeconds * 1000 - Date.now()) / 60000))
  if (diffMinutes <= 0) return locale === 'zh-CN' ? '马上' : 'soon'
  if (diffMinutes >= 60 * 24 * 7) {
    const days = Math.ceil(diffMinutes / (60 * 24))
    return locale === 'zh-CN' ? `${days}天` : `${days}d`
  }
  if (diffMinutes >= 60 * 24) {
    const days = Math.ceil(diffMinutes / (60 * 24))
    const hours = Math.floor((diffMinutes % (60 * 24)) / 60)
    return hours > 0 ? `${days}d${hours}h` : `${days}d`
  }
  if (diffMinutes >= 60) {
    const hours = Math.floor(diffMinutes / 60)
    const minutes = diffMinutes % 60
    return minutes > 0 ? `${hours}h${minutes}m` : `${hours}h`
  }
  return `${diffMinutes}m`
}

const quotaPeriodLabel = (kind, locale) => {
  if (kind === 'short_quota') return locale === 'zh-CN' ? '5小时额度' : '5h quota'
  if (kind === 'weekly_quota') return locale === 'zh-CN' ? '7天额度' : '7d quota'
  if (kind === 'total_24h_tokens') return locale === 'zh-CN' ? '24小时用量' : '24h usage'
  if (kind === 'total_7d_tokens') return locale === 'zh-CN' ? '7天用量' : '7d usage'
  if (kind === 'last_tokens') return locale === 'zh-CN' ? '最近一次' : 'last'
  if (kind === 'usage_tokens') return locale === 'zh-CN' ? '本次用量' : 'tokens'
  return locale === 'zh-CN' ? '用量' : 'usage'
}

const compactUsageValue = (value) => {
  if (!value) return '--'
  return String(value).replace(/\s+/g, '')
}

const normalizeDetailPart = (part, locale) => {
  const text = String(part || '').trim()
  if (locale !== 'zh-CN') return text
  return text
    .replace(/^in\s+/i, '输入 ')
    .replace(/^out\s+/i, '输出 ')
    .replace(/^cache\s+/i, '缓存 ')
}

const usageRowFromMetric = (metric, locale) => {
  const kind = metric.meta?.kind || 'usage'
  if (kind === 'total_tokens') return null
  const isTokenRow = kind === 'total_24h_tokens'
    || kind === 'total_7d_tokens'
    || kind === 'last_tokens'
    || kind === 'usage_tokens'
  const rawRemaining = metric.meta?.remainingPercent ?? metric.percent
  const rawUsed = metric.meta?.usedPercent
  const remaining = rawRemaining == null || rawRemaining === '' ? null : Number(rawRemaining)
  const used = rawUsed == null || rawUsed === '' ? null : Number(rawUsed)
  const resetLabel = formatResetLabel(metric.meta?.resetsAt, locale)
  const detailParts = String(metric.detail || '')
    .split(' · ')
    .map((part) => part.trim())
    .filter(Boolean)
  const valueText = !isTokenRow && Number.isFinite(remaining)
    ? `${Math.round(remaining)}%`
    : compactUsageValue(metric.value)
  const tokenDetails = detailParts.map((part) => normalizeDetailPart(part, locale))
  const cacheDetail = tokenDetails.find((part) => (
    part.toLowerCase().startsWith('cache') || part.startsWith('缓存')
  )) || ''
  const mainTokenDetails = tokenDetails
    .filter((part) => part !== cacheDetail)
    .slice(0, 2)
  const noteText = isTokenRow
    ? mainTokenDetails.join(' · ')
    : ''
  const detailText = isTokenRow
    ? cacheDetail
    : (resetLabel || (Number.isFinite(used)
      ? (locale === 'zh-CN' ? `已用 ${Math.round(used)}%` : `${Math.round(used)}% used`)
      : metric.detail || '--'))

  return {
    id: metric.id,
    kind,
    period: quotaPeriodLabel(kind, locale),
    valueText,
    noteText,
    detailText,
    percent: !isTokenRow && Number.isFinite(remaining) ? Math.max(0, Math.min(remaining, 100)) : null,
    status: metric.status || 'info',
    isTokenRow,
  }
}

const usageRowOrder = {
  short_quota: 0,
  weekly_quota: 1,
  total_7d_tokens: 2,
  total_24h_tokens: 3,
  last_tokens: 4,
  usage_tokens: 5,
  usage: 6,
}

const groupUsageMetrics = (metrics, locale) => {
  const grouped = new Map()

  metrics.forEach((metric) => {
    const source = metric.source || 'agent'
    const group = grouped.get(source) || {
      id: source,
      title: metric.sourceLabel || metric.source || 'Agent',
      rows: [],
      hasQuota: false,
    }
    const row = usageRowFromMetric(metric, locale)
    if (!row) return
    group.rows.push(row)
    if (metric.meta?.kind === 'short_quota' || metric.meta?.kind === 'weekly_quota') {
      group.hasQuota = true
    }
    if (!group.title && metric.sourceLabel) group.title = metric.sourceLabel
    grouped.set(source, group)
  })

  return Array.from(grouped.values()).map((group) => ({
    ...group,
    syncText: group.hasQuota
      ? (locale === 'zh-CN' ? '已同步' : 'Synced')
      : '',
    rows: group.rows.sort((a, b) => (
      (usageRowOrder[a.kind] ?? usageRowOrder.usage) - (usageRowOrder[b.kind] ?? usageRowOrder.usage)
    )),
  }))
}

function ContextGearIcon() {
  return (
    <svg className="context-icon" viewBox="0 0 24 24" aria-hidden="true">
      <path
        d="M12 3.2v3M12 17.8v3M4.2 12h3M16.8 12h3M6.5 6.5l2.1 2.1M15.4 15.4l2.1 2.1M17.5 6.5l-2.1 2.1M8.6 15.4l-2.1 2.1"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="3"
      />
      <circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" strokeWidth="3" />
    </svg>
  )
}

function ContextGaugeIcon() {
  return (
    <svg className="context-icon" viewBox="0 0 24 24" aria-hidden="true">
      <path
        d="M4.8 14.4a7.2 7.2 0 0 1 14.4 0"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="3"
      />
      <path d="M12 14.2l4.1-4.1" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="3" />
      <circle cx="12" cy="14.2" r="1.9" fill="currentColor" />
    </svg>
  )
}

function PanelPinIcon() {
  return (
    <svg className="panel-pin-icon" viewBox="0 0 24 24" aria-hidden="true">
      <path
        d="M8.2 4.6h7.6l-1 5.4 3.2 3.1v1.5H13l-1 5.2h-1l-1-5.2H6v-1.5l3.2-3.1-1-5.4Z"
        fill="currentColor"
      />
      <path d="M9.6 4.6h4.8" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="2.4" />
    </svg>
  )
}

function PetWindow({
  bubble,
  currentState,
  currentFrame,
  displayHeight,
  displayWidth,
  getBackgroundPosition,
  handleBubbleClick,
  handleNoticeAction,
  handleNoticeDismiss,
  handleOpenSettings,
  handlePetMouseDown,
  handleTrigger,
  handleToggleUsageDashboard,
  handleToggleUsageDashboardPinned,
  handleUsageDashboardActivity,
  notices,
  noticeOnly = false,
  petScale,
  spritesheet,
  t,
  usageDashboardPinned,
  usageDashboardVisible,
  usageMetrics,
  windowHeight,
  windowWidth,
}) {
  const visibleMetrics = usageMetrics.slice(0, MAX_VISIBLE_USAGE_METRICS)
  const usageGroups = useMemo(
    () => groupUsageMetrics(visibleMetrics, t('localeCode')),
    [t, visibleMetrics],
  )
  const visibleNotices = notices.slice(0, 8)
  const hiddenNoticeCount = Math.max(0, notices.length - visibleNotices.length)
  const petTop = windowHeight - displayHeight
  const noticePanelHeight = noticeOnly
    ? Math.max(0, windowHeight - 8)
    : clampNumber(
      Math.min(PANEL_DEFAULT_SIZES.notices.height, petTop - NOTICE_TOP_GAP - 8),
      Math.min(NOTICE_PANEL_MIN_HEIGHT, Math.max(0, petTop - NOTICE_TOP_GAP - 8)),
      PANEL_DEFAULT_SIZES.notices.height,
    )
  const noticeScrollHeight = noticeOnly
    ? Math.max(72, noticePanelHeight - 8)
    : Math.max(72, noticePanelHeight - NOTICE_TITLE_HEIGHT - NOTICE_STACK_GAP)
  const stackColors = ['yellow', 'mint', 'peach']
  const [panelPositions, setPanelPositions] = useState(readStoredPanelPositions)
  const [panelSizes, setPanelSizes] = useState(readStoredPanelSizes)
  const [draggingPanel, setDraggingPanel] = useState(null)
  const [resizingPanel, setResizingPanel] = useState(null)
  const [contextMenu, setContextMenu] = useState(null)
  const dragRef = useRef(null)
  const resizeRef = useRef(null)

  const resolvePanelSize = useCallback((panel) => {
    const defaultSize = PANEL_DEFAULT_SIZES[panel]
    const storedSize = panelSizes[panel] || {}
    if (!defaultSize) return { width: 0, height: 0 }

    if (panel === 'notices') {
      return { width: defaultSize.width, height: noticePanelHeight }
    }

    const limits = PANEL_SIZE_LIMITS[panel]
    if (!limits) return defaultSize

    const margin = 8
    const maxWidth = Math.max(limits.minWidth, Math.min(limits.maxWidth, windowWidth - margin * 2))
    const maxHeight = Math.max(limits.minHeight, Math.min(limits.maxHeight, windowHeight - margin * 2))

    return {
      width: clampNumber(Number(storedSize.width) || defaultSize.width, limits.minWidth, maxWidth),
      height: clampNumber(Number(storedSize.height) || defaultSize.height, limits.minHeight, maxHeight),
    }
  }, [noticePanelHeight, panelSizes, windowHeight, windowWidth])

  const defaultPanelPositions = useMemo(() => {
    const headX = windowWidth / 2
    const topGap = 42
    const sideGap = 12
    const noticeWidth = PANEL_DEFAULT_SIZES.notices.width

    return {
      usage: {
        x: headX - resolvePanelSize('usage').width - sideGap,
        y: petTop - resolvePanelSize('usage').height - topGap,
      },
      notices: noticeOnly
        ? { x: 10, y: Math.max(8, windowHeight - noticePanelHeight - 8) }
        : {
          x: headX - noticeWidth / 2,
          y: petTop - noticePanelHeight - NOTICE_TOP_GAP,
        },
    }
  }, [noticeOnly, noticePanelHeight, petTop, resolvePanelSize, windowWidth])

  const clampPanelPosition = useCallback((panel, position, sizeOverride) => {
    const size = sizeOverride || resolvePanelSize(panel)
    const margin = 8
    const maxY = panel === 'notices' && !noticeOnly
      ? Math.max(margin, petTop - size.height - NOTICE_TOP_GAP)
      : Math.max(margin, windowHeight - size.height - margin)

    return {
      x: Math.min(Math.max(position.x, margin), Math.max(margin, windowWidth - size.width - margin)),
      y: Math.min(Math.max(position.y, margin), maxY),
    }
  }, [noticeOnly, petTop, resolvePanelSize, windowHeight, windowWidth])

  const resolvePanelPosition = useCallback((panel) => (
    clampPanelPosition(panel, panelPositions[panel] || defaultPanelPositions[panel])
  ), [clampPanelPosition, defaultPanelPositions, panelPositions])

  const resolveResizedPanelFrame = useCallback((resize, mouse) => {
    const limits = PANEL_SIZE_LIMITS[resize.panel]
    if (!limits) {
      return {
        position: resize.startPosition,
        size: resize.startSize,
      }
    }

    const margin = 8
    const deltaX = mouse.x - resize.startMouse.x
    const deltaY = mouse.y - resize.startMouse.y
    const right = resize.startPosition.x + resize.startSize.width
    const bottom = resize.startPosition.y + resize.startSize.height
    let x = resize.startPosition.x
    let y = resize.startPosition.y
    let width = resize.startSize.width
    let height = resize.startSize.height

    if (resize.edge === 'right') {
      const maxWidth = Math.max(limits.minWidth, Math.min(limits.maxWidth, windowWidth - x - margin))
      width = clampNumber(resize.startSize.width + deltaX, limits.minWidth, maxWidth)
    }

    if (resize.edge === 'left') {
      const maxWidth = Math.max(limits.minWidth, Math.min(limits.maxWidth, right - margin))
      width = clampNumber(resize.startSize.width - deltaX, limits.minWidth, maxWidth)
      x = right - width
    }

    if (resize.edge === 'bottom') {
      const maxHeight = Math.max(limits.minHeight, Math.min(limits.maxHeight, windowHeight - y - margin))
      height = clampNumber(resize.startSize.height + deltaY, limits.minHeight, maxHeight)
    }

    if (resize.edge === 'top') {
      const maxHeight = Math.max(limits.minHeight, Math.min(limits.maxHeight, bottom - margin))
      height = clampNumber(resize.startSize.height - deltaY, limits.minHeight, maxHeight)
      y = bottom - height
    }

    const size = { width, height }

    return {
      position: clampPanelPosition(resize.panel, { x, y }, size),
      size,
    }
  }, [clampPanelPosition, windowHeight, windowWidth])

  useEffect(() => {
    try {
      window.localStorage.setItem(PANEL_POSITION_STORAGE_KEY, JSON.stringify(panelPositions))
    } catch {
      // Position persistence is a small convenience; ignore storage failures.
    }
  }, [panelPositions])

  useEffect(() => {
    try {
      window.localStorage.setItem(PANEL_SIZE_STORAGE_KEY, JSON.stringify(panelSizes))
    } catch {
      // Size persistence is a small convenience; ignore storage failures.
    }
  }, [panelSizes])

  useEffect(() => {
    if (!draggingPanel) return undefined

    const handleMouseMove = (event) => {
      const drag = dragRef.current
      if (!drag) return

      const nextPosition = clampPanelPosition(drag.panel, {
        x: drag.startPosition.x + event.clientX - drag.startMouse.x,
        y: drag.startPosition.y + event.clientY - drag.startMouse.y,
      })
      setPanelPositions((prev) => ({ ...prev, [drag.panel]: nextPosition }))
    }

    const handleMouseUp = () => {
      dragRef.current = null
      setDraggingPanel(null)
      document.body.classList.remove('pet-pointer-capture')
    }

    window.addEventListener('mousemove', handleMouseMove)
    window.addEventListener('mouseup', handleMouseUp)

    return () => {
      window.removeEventListener('mousemove', handleMouseMove)
      window.removeEventListener('mouseup', handleMouseUp)
      document.body.classList.remove('pet-pointer-capture')
    }
  }, [clampPanelPosition, draggingPanel])

  useEffect(() => {
    if (!resizingPanel) return undefined

    const handleMouseMove = (event) => {
      const resize = resizeRef.current
      if (!resize) return

      const nextFrame = resolveResizedPanelFrame(resize, {
        x: event.clientX,
        y: event.clientY,
      })

      setPanelSizes((prev) => ({ ...prev, [resize.panel]: nextFrame.size }))
      setPanelPositions((prev) => ({
        ...prev,
        [resize.panel]: nextFrame.position,
      }))
      if (resize.panel === 'usage') {
        handleUsageDashboardActivity?.()
      }
    }

    const handleMouseUp = () => {
      resizeRef.current = null
      setResizingPanel(null)
      document.body.classList.remove('pet-pointer-capture')
    }

    window.addEventListener('mousemove', handleMouseMove)
    window.addEventListener('mouseup', handleMouseUp)

    return () => {
      window.removeEventListener('mousemove', handleMouseMove)
      window.removeEventListener('mouseup', handleMouseUp)
      document.body.classList.remove('pet-pointer-capture')
    }
  }, [handleUsageDashboardActivity, resolveResizedPanelFrame, resizingPanel])

  useEffect(() => {
    if (!contextMenu) return undefined

    const handleKeyDown = (event) => {
      if (event.key === 'Escape') {
        setContextMenu(null)
      }
    }

    window.addEventListener('keydown', handleKeyDown)

    return () => {
      window.removeEventListener('keydown', handleKeyDown)
    }
  }, [contextMenu])

  const handlePanelMouseDown = (panel, event) => {
    if (event.button !== 0) return
    if (event.target.closest?.('button, input, select, textarea, a')) return

    document.body.classList.add('pet-pointer-capture')
    if (panel === 'usage') {
      handleUsageDashboardActivity?.()
    }
    setContextMenu(null)
    event.stopPropagation()
    dragRef.current = {
      panel,
      startMouse: { x: event.clientX, y: event.clientY },
      startPosition: resolvePanelPosition(panel),
    }
    setDraggingPanel(panel)
  }

  const handlePanelResizeMouseDown = (panel, edge, event) => {
    if (event.button !== 0) return
    event.preventDefault()
    event.stopPropagation()
    document.body.classList.add('pet-pointer-capture')
    setContextMenu(null)
    resizeRef.current = {
      panel,
      edge,
      startMouse: { x: event.clientX, y: event.clientY },
      startPosition: resolvePanelPosition(panel),
      startSize: resolvePanelSize(panel),
    }
    setResizingPanel({ panel, edge })
    if (panel === 'usage') {
      handleUsageDashboardActivity?.()
    }
  }

  const handleContextMenu = (event) => {
    event.preventDefault()
    event.stopPropagation()

    const margin = 5
    const petLeft = (windowWidth - displayWidth) / 2
    const petRight = petLeft + displayWidth
    const petTop = windowHeight - displayHeight
    const preferredX = petRight - 2
    const preferredY = petTop + (displayHeight - CONTEXT_MENU_SIZE.height) / 2

    setContextMenu({
      x: Math.min(Math.max(preferredX, margin), Math.max(margin, windowWidth - CONTEXT_MENU_SIZE.width - margin)),
      y: Math.min(Math.max(preferredY, margin), Math.max(margin, windowHeight - CONTEXT_MENU_SIZE.height - margin)),
    })
  }

  const handleContainerClick = () => {
    if (contextMenu) {
      setContextMenu(null)
      return
    }

    handleTrigger('jumping')
  }

  const handleUsageDashboardMenuClick = (event) => {
    event.stopPropagation()
    setContextMenu(null)
    handleToggleUsageDashboard()
  }

  const handleUsageDashboardPinClick = (event) => {
    event.stopPropagation()
    handleToggleUsageDashboardPinned()
  }

  const handleSettingsMenuClick = (event) => {
    event.stopPropagation()
    setContextMenu(null)
    handleOpenSettings()
  }

  const usagePosition = resolvePanelPosition('usage')
  const usageSize = resolvePanelSize('usage')
  const noticePosition = resolvePanelPosition('notices')

  return (
    <div
      className={noticeOnly ? 'pet-container notice-window-container' : 'pet-container'}
      style={{ width: windowWidth, height: windowHeight, '--pet-display-height': `${displayHeight}px` }}
      onClick={noticeOnly ? undefined : handleContainerClick}
      onContextMenu={noticeOnly ? undefined : handleContextMenu}
    >
      {!noticeOnly && contextMenu && (
        <div
          className="pet-context-menu"
          onMouseDown={(event) => event.stopPropagation()}
          onClick={(event) => event.stopPropagation()}
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <button
            type="button"
            className="pet-context-menu-button primary"
            onClick={handleSettingsMenuClick}
            aria-label={t('settings')}
            title={t('settings')}
          >
            <ContextGearIcon />
          </button>
          <button
            type="button"
            className="pet-context-menu-button"
            onClick={handleUsageDashboardMenuClick}
            aria-label={usageDashboardVisible ? t('hideUsageDashboard') : t('showUsageDashboard')}
            title={usageDashboardVisible ? t('hideUsageDashboard') : t('showUsageDashboard')}
          >
            <ContextGaugeIcon />
          </button>
        </div>
      )}

      {!noticeOnly && usageDashboardVisible && (
        <div
          className={[
            'usage-dashboard floating-panel',
            draggingPanel === 'usage' ? 'dragging' : '',
            resizingPanel?.panel === 'usage' ? `resizing resizing-${resizingPanel.edge}` : '',
          ].filter(Boolean).join(' ')}
          onMouseDown={(event) => handlePanelMouseDown('usage', event)}
          onClick={(event) => event.stopPropagation()}
          style={{
            left: usagePosition.x,
            top: usagePosition.y,
            width: usageSize.width,
            height: usageSize.height,
            '--usage-dashboard-body-height': `${Math.max(74, usageSize.height - 60)}px`,
          }}
        >
          <div className="panel-label">
            <span className="panel-label-icon">▥</span>
            <span>{t('usageDashboard')}</span>
            <button
              type="button"
              className={`panel-pin-button ${usageDashboardPinned ? 'pinned' : ''}`}
              onMouseDown={(event) => event.stopPropagation()}
              onClick={handleUsageDashboardPinClick}
              aria-pressed={usageDashboardPinned}
              aria-label={usageDashboardPinned ? t('unpinUsageDashboard') : t('pinUsageDashboard')}
              title={usageDashboardPinned ? t('unpinUsageDashboard') : t('pinUsageDashboard')}
            >
              <PanelPinIcon />
            </button>
          </div>
          {visibleMetrics.length > 0 ? (
            <div className="usage-groups">
              {usageGroups.map((group) => (
                <div className="usage-group" key={group.id}>
                  <div className="usage-group-header">
                    <span>{group.title}</span>
                    {group.syncText && <span>{group.syncText}</span>}
                  </div>
                  <div className="usage-rows">
                    {group.rows.map((row) => {
                      const barPercent = row.percent ?? 0
                      const rowClass = [
                        'usage-row',
                        row.isTokenRow ? 'usage-row-token' : '',
                        `usage-${row.status || 'info'}`,
                      ].filter(Boolean).join(' ')
                      return (
                        <div className={rowClass} key={row.id}>
                          <div className="usage-row-period">{row.period}</div>
                          {row.percent == null ? (
                            <div className="usage-row-note">{row.noteText || row.detailText || '--'}</div>
                          ) : (
                            <div
                              className="usage-row-bar"
                              style={{ '--usage-row-percent': `${barPercent}%` }}
                              aria-hidden="true"
                            >
                              <span />
                            </div>
                          )}
                          <div className="usage-row-value">{row.valueText}</div>
                          <div className="usage-row-detail">{row.detailText || '--'}</div>
                        </div>
                      )
                    })}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="usage-empty">
              <div className="usage-empty-ring" aria-hidden="true">▥</div>
              <div>
                <div className="usage-empty-title">{t('usageWaiting')}</div>
                <div className="usage-empty-detail">{t('usageWaitingDetail')}</div>
              </div>
            </div>
          )}
          {PANEL_RESIZE_EDGES.map((edge) => (
            <div
              className={`panel-resize-zone panel-resize-${edge}`}
              key={edge}
              onMouseDown={(event) => handlePanelResizeMouseDown('usage', edge, event)}
              aria-hidden="true"
            />
          ))}
        </div>
      )}

      {visibleNotices.length > 0 && (
        <div
          className={`notice-stack floating-panel ${draggingPanel === 'notices' ? 'dragging' : ''}`}
          onMouseDown={noticeOnly ? (event) => event.stopPropagation() : (event) => handlePanelMouseDown('notices', event)}
          onClick={(event) => event.stopPropagation()}
          style={{ left: noticePosition.x, top: noticePosition.y, height: noticePanelHeight }}
        >
          {!noticeOnly && <div className="stack-title">{t('notes')}</div>}
          <div
            className="notice-scroll"
            style={{ '--notice-scroll-height': `${noticeScrollHeight}px` }}
          >
            {visibleNotices.map((item, index) => {
              const value = item.value || item.detail || ''
              const hasAction = item.focusSource || item.actionKind || item.actionLabel || item.actionHint
              const noticeSuffix = noticeTranslationKeySuffix(item.noticeType || 'info')
              const displayTitle = item.title || t(`noticeTitle${noticeSuffix}`)
              const displayActionHint = item.actionHint || t(`noticeHint${noticeSuffix}`)
              return (
                <div
                  className={`sticky-notice sticky-${item.level || 'info'} sticky-type-${item.noticeType || 'info'} sticky-paper-${stackColors[index % stackColors.length]}`}
                  key={item.id}
                  style={{ '--stack-index': index }}
                  title={item.sourceLabel || item.source}
                >
                  <div className="sticky-header">
                    <span className="sticky-pin" aria-hidden="true">{noticeIcon[item.level] || noticeIcon.info}</span>
                    <div className="sticky-source">{item.sourceLabel || item.source || 'Agent Pet'}</div>
                    <div className="sticky-type">{noticeTypeLabel(item.noticeType || 'info', t('localeCode'))}</div>
                    <button
                      type="button"
                      className="sticky-close"
                      onMouseDown={(event) => event.stopPropagation()}
                      onClick={(event) => handleNoticeDismiss(event, item)}
                      aria-label={t('dismissNotice')}
                    >
                      {t('markNoticeRead')}
                    </button>
                  </div>
                  <div className="sticky-main">
                    <div className="sticky-copy">
                      <div className="sticky-title">{displayTitle}</div>
                      {value && <div className="sticky-value">{value}</div>}
                      {item.body && <div className="sticky-body">{item.body}</div>}
                      {(displayActionHint || hasAction) && (
                        <div className="sticky-action-row">
                          {displayActionHint && <span className="sticky-action-hint">{displayActionHint}</span>}
                          {hasAction && item.source && handleNoticeAction && (
                            <button
                              type="button"
                              className="sticky-action-button"
                              onMouseDown={(event) => event.stopPropagation()}
                              onClick={(event) => handleNoticeAction(event, item)}
                            >
                              {item.actionLabel || t('noticeActionReview')}
                            </button>
                          )}
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              )
            })}
            {hiddenNoticeCount > 0 && (
              <div className="sticky-overflow">{t('moreNotices', { count: hiddenNoticeCount })}</div>
            )}
          </div>
        </div>
      )}

      {!noticeOnly && bubble?.text && (
        <div
          className={`pet-bubble ${bubble.source ? 'clickable' : ''}`}
          onMouseDown={(event) => event.stopPropagation()}
          onClick={handleBubbleClick}
          title={bubble.sourceLabel || bubble.source}
        >
          {bubble.text}
        </div>
      )}
      {!noticeOnly && spritesheet ? (
        <div
          className="pet-sprite"
          onMouseDown={handlePetMouseDown}
          style={{
            backgroundImage: `url(${spritesheet})`,
            backgroundPosition: getBackgroundPosition(),
            backgroundSize: `${ATLAS_WIDTH * petScale}px ${ATLAS_HEIGHT * petScale}px`,
            width: displayWidth,
            height: displayHeight,
          }}
        />
      ) : !noticeOnly ? (
        <div
          className="pet-placeholder"
          onMouseDown={handlePetMouseDown}
          style={{
            width: displayWidth,
            height: displayHeight,
          }}
        >
          {t('noLoaded')}
        </div>
      ) : null}
    </div>
  )
}

export default PetWindow
