import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { emit, listen } from '@tauri-apps/api/event'
import { cursorPosition, getCurrentWindow, Window } from '@tauri-apps/api/window'
import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi'
import { t as translate } from './locales'
import PetWindow from './PetWindow'
import SettingsWindow from './SettingsWindow'
import {
  BUBBLE_SPACE_HEIGHT,
  CELL_HEIGHT,
  CELL_WIDTH,
  DEFAULT_PET_SCALE,
  EMPTY_LIVE_SOURCE_STATUS,
  FRAME_COUNTS,
  MIN_WINDOW_WIDTH,
  PET_STAGE_EXTRA_HEIGHT,
  PET_STAGE_WIDTH,
  STATE_ROWS,
} from './constants'

const getSpritesheetSource = (petConfig) => {
  return petConfig?.spritesheetDataUrl || petConfig?.spritesheet_data_url || ''
}

const USAGE_DASHBOARD_PINNED_KEY = 'agent-pet-usage-dashboard-pinned-v1'
const USAGE_DASHBOARD_AUTO_HIDE_MS = 9000
const MAX_USAGE_METRICS = 24
const POINTER_PASSTHROUGH_POLL_MS = 80
const POINTER_HIT_PADDING = 24
const POINTER_HIT_SELECTORS = [
  '.pet-sprite',
  '.pet-placeholder',
  '.pet-bubble',
  '.pet-context-menu',
  '.usage-dashboard',
  '.notice-stack',
]
const NOTICE_WINDOW_WIDTH = 316
const NOTICE_WINDOW_HEIGHT = 332
const NOTICE_WINDOW_GAP = 6
const NOTICE_CARD_HEIGHT = 124
const NOTICE_CARD_GAP = 8
const NOTICE_WINDOW_VERTICAL_PADDING = 16
const PET_GEOMETRY_STORAGE_KEY = 'agent-pet-current-geometry-v1'
const NOTICE_WINDOW_MANUAL_KEY = 'agent-pet-notice-window-manual-v1'

const isPointInsideVisiblePetSurface = (x, y) => {
  if (document.body.classList.contains('pet-pointer-capture')) return true

  return POINTER_HIT_SELECTORS.some((selector) => (
    Array.from(document.querySelectorAll(selector)).some((element) => {
      const rect = element.getBoundingClientRect()
      if (rect.width <= 0 || rect.height <= 0) return false

      return x >= rect.left - POINTER_HIT_PADDING
        && x <= rect.right + POINTER_HIT_PADDING
        && y >= rect.top - POINTER_HIT_PADDING
        && y <= rect.bottom + POINTER_HIT_PADDING
    })
  ))
}

const cursorLocalPointCandidates = (cursor, position, scaleFactor) => {
  const scale = Number(scaleFactor) > 0 ? Number(scaleFactor) : 1
  return [
    {
      x: (cursor.x - position.x) / scale,
      y: (cursor.y - position.y) / scale,
    },
    {
      x: cursor.x - position.x,
      y: cursor.y - position.y,
    },
    {
      x: cursor.x / scale - position.x,
      y: cursor.y / scale - position.y,
    },
    {
      x: cursor.x - position.x / scale,
      y: cursor.y - position.y / scale,
    },
  ]
}

const readStoredUsageDashboardPinned = () => {
  try {
    return window.localStorage.getItem(USAGE_DASHBOARD_PINNED_KEY) === 'true'
  } catch {
    return false
  }
}

const normalizeNotice = (payload = {}) => ({
  id: payload.id || payload.groupKey || `${payload.source || 'notice'}-${Date.now()}`,
  groupKey: payload.groupKey || payload.id || '',
  level: payload.level || 'info',
  noticeType: payload.noticeType || payload.notice_type || 'info',
  title: payload.title || 'Notice',
  body: payload.body || '',
  value: payload.value || '',
  detail: payload.detail || '',
  art: payload.art || '',
  source: payload.source || '',
  sourceLabel: payload.sourceLabel || '',
  actionHint: payload.actionHint || payload.action_hint || '',
  actionLabel: payload.actionLabel || payload.action_label || '',
  focusSource: Boolean(payload.focusSource || payload.focus_source),
  actionKind: payload.actionKind || payload.action_kind || '',
  automationSafe: Boolean(payload.automationSafe || payload.automation_safe),
  ttlSeconds: Number.isFinite(Number(payload.ttlSeconds)) ? Number(payload.ttlSeconds) : null,
  receivedAt: Date.now(),
})

const normalizeUsageMetric = (payload = {}) => ({
  id: payload.id || payload.groupKey || `${payload.source || 'usage'}-${payload.label || Date.now()}`,
  source: payload.source || '',
  sourceLabel: payload.sourceLabel || payload.source || '',
  label: payload.label || payload.title || 'Usage',
  value: payload.value || '',
  detail: payload.detail || payload.body || '',
  percent: Number.isFinite(Number(payload.percent)) ? Math.max(0, Math.min(Number(payload.percent), 100)) : null,
  status: payload.status || payload.level || 'info',
  meta: payload.meta && typeof payload.meta === 'object' ? payload.meta : {},
})

const listenSafe = async (eventName, handler) => {
  try {
    const unlisten = await listen(eventName, handler)
    return () => {
      try {
        unlisten()
      } catch (e) {
        console.warn(`Failed to unlisten ${eventName}:`, e)
      }
    }
  } catch (e) {
    console.error(`Failed to listen for ${eventName}:`, e)
    return () => {}
  }
}

const cleanupListener = (listenerPromise) => {
  Promise.resolve(listenerPromise)
    .then((unlisten) => {
      if (typeof unlisten === 'function') unlisten()
    })
    .catch((e) => {
      console.warn('Failed to cleanup listener:', e)
    })
}

function App() {
  const [currentState, setCurrentState] = useState('idle')
  const [currentFrame, setCurrentFrame] = useState(0)
  const [spritesheet, setSpritesheet] = useState('')
  const [windowLabel, setWindowLabel] = useState('')
  const [pets, setPets] = useState([])
  const [petLibrary, setPetLibrary] = useState([])
  const [petLibraryPage, setPetLibraryPage] = useState({
    page: 1,
    pageSize: 30,
    total: 0,
    totalPages: 1,
    fromCache: false,
  })
  const [petLibraryStatus, setPetLibraryStatus] = useState('idle')
  const [installingPetId, setInstallingPetId] = useState('')
  const [messageMap, setMessageMap] = useState({})
  const [wsStatus, setWsStatus] = useState({ enabled: true, port: 8765 })
  const [liveSourcesStatus, setLiveSourcesStatus] = useState({
    codex: { ...EMPTY_LIVE_SOURCE_STATUS },
    claude: { ...EMPTY_LIVE_SOURCE_STATUS },
    opencode: { ...EMPTY_LIVE_SOURCE_STATUS },
    openclaw: { ...EMPTY_LIVE_SOURCE_STATUS },
    hermes: { ...EMPTY_LIVE_SOURCE_STATUS },
    antigravity: { ...EMPTY_LIVE_SOURCE_STATUS },
  })
  const [liveSourcePrefixEnabled, setLiveSourcePrefixEnabled] = useState(false)
  const [petScale, setPetScale] = useState(DEFAULT_PET_SCALE)
  const [bubble, setBubble] = useState(null)
  const [notices, setNotices] = useState([])
  const [usageMetrics, setUsageMetrics] = useState([])
  const [usageDashboardPinned, setUsageDashboardPinned] = useState(readStoredUsageDashboardPinned)
  const [usageDashboardTemporaryVisible, setUsageDashboardTemporaryVisible] = useState(false)
  const [userPetDir, setUserPetDir] = useState('')
  const [locale, setLocale] = useState('en')
  const [bootError, setBootError] = useState('')

  const t = (key, params) => translate(locale, key, params)
  
  const animRef = useRef(null)
  const bubbleTimerRef = useRef(null)
  const lastTimeRef = useRef(0)
  const frameRef = useRef(0)
  const stateRef = useRef('idle')
  const stateStartRef = useRef(Date.now())
  const windowFrameRef = useRef(null)
  const petGeometryRef = useRef(null)
  const usageDashboardPinnedRef = useRef(usageDashboardPinned)
  const usageDashboardAutoHideTimerRef = useRef(null)
  const displayWidth = CELL_WIDTH * petScale
  const displayHeight = CELL_HEIGHT * petScale
  const usageDashboardVisible = usageDashboardPinned || usageDashboardTemporaryVisible
  const visibleUsageMetrics = usageDashboardVisible ? usageMetrics : []
  const hasPetPanels = usageDashboardVisible
  const windowWidth = Math.max(displayWidth, MIN_WINDOW_WIDTH, hasPetPanels ? PET_STAGE_WIDTH : 0)
  const windowHeight = displayHeight + BUBBLE_SPACE_HEIGHT + (hasPetPanels ? PET_STAGE_EXTRA_HEIGHT : 0)

  const clearUsageDashboardAutoHide = () => {
    if (!usageDashboardAutoHideTimerRef.current) return
    clearTimeout(usageDashboardAutoHideTimerRef.current)
    usageDashboardAutoHideTimerRef.current = null
  }

  const scheduleUsageDashboardAutoHide = () => {
    clearUsageDashboardAutoHide()
    usageDashboardAutoHideTimerRef.current = setTimeout(() => {
      if (!usageDashboardPinnedRef.current) {
        setUsageDashboardTemporaryVisible(false)
      }
      usageDashboardAutoHideTimerRef.current = null
    }, USAGE_DASHBOARD_AUTO_HIDE_MS)
  }

  const showUsageDashboardTemporarily = () => {
    if (usageDashboardPinnedRef.current) return
    setUsageDashboardTemporaryVisible(true)
    scheduleUsageDashboardAutoHide()
  }

  const publishPetGeometry = async () => {
    try {
      if (windowLabel !== 'pet') return
      const sprite = document.querySelector('.pet-sprite')
      if (!sprite) return
      const rect = sprite.getBoundingClientRect()
      const petWindow = getCurrentWindow()
      const [position, scaleFactor] = await Promise.all([
        petWindow.outerPosition(),
        petWindow.scaleFactor(),
      ])
      const windowPosition = position.toLogical(scaleFactor)
      const geometry = {
        left: windowPosition.x + rect.left,
        top: windowPosition.y + rect.top,
        width: rect.width,
        height: rect.height,
        centerX: windowPosition.x + rect.left + rect.width / 2,
      }
      try {
        window.localStorage.setItem(PET_GEOMETRY_STORAGE_KEY, JSON.stringify(geometry))
      } catch {
        // Geometry events still keep the live positioning usable.
      }
      await emit('pet-geometry', geometry)
    } catch (e) {
      console.error('Failed to publish pet geometry:', e)
    }
  }

  useEffect(() => {
    if (windowLabel !== 'pet') return

    const resizePetWindow = async () => {
      try {
        const petWindow = getCurrentWindow()
        const scaleFactor = await petWindow.scaleFactor()
        const position = await petWindow.outerPosition()
        const previousHeight = windowFrameRef.current?.height ?? windowHeight
        const nextPositionY = position.y + (previousHeight - windowHeight) * scaleFactor

        await petWindow.setSize(new LogicalSize(windowWidth, windowHeight))
        if (Math.abs(nextPositionY - position.y) > 0.5) {
          await petWindow.setPosition(new LogicalPosition(position.x / scaleFactor, nextPositionY / scaleFactor))
        }

        windowFrameRef.current = { width: windowWidth, height: windowHeight }
        requestAnimationFrame(publishPetGeometry)
      } catch (e) {
        console.error('Failed to resize pet window:', e)
      }
    }

    resizePetWindow()
  }, [windowHeight, windowLabel, windowWidth, displayHeight, displayWidth])

  useEffect(() => {
    if (windowLabel !== 'pet' || !spritesheet) return

    requestAnimationFrame(publishPetGeometry)
    const timer = window.setInterval(publishPetGeometry, 500)
    const unlistenGeometryRequest = listenSafe('request-pet-geometry', () => {
      requestAnimationFrame(publishPetGeometry)
    })

    return () => {
      window.clearInterval(timer)
      cleanupListener(unlistenGeometryRequest)
    }
  }, [displayHeight, displayWidth, petScale, spritesheet, windowLabel])

  useEffect(() => {
    if (windowLabel !== 'notices') return

    const readPetGeometry = () => {
      if (petGeometryRef.current) return petGeometryRef.current
      try {
        const geometry = JSON.parse(window.localStorage.getItem(PET_GEOMETRY_STORAGE_KEY) || 'null')
        if (geometry && typeof geometry === 'object') return geometry
      } catch {
        // Fall back to the pet window bounds below.
      }
      return null
    }

    const positionNoticeWindow = async () => {
      try {
        const noticeWindow = getCurrentWindow()
        const hasNotices = notices.length > 0
        const visibleNoticeCount = Math.max(1, Math.min(notices.length, 2))
        const noticeHeight = hasNotices
          ? Math.min(
            NOTICE_WINDOW_HEIGHT,
            visibleNoticeCount * NOTICE_CARD_HEIGHT
              + Math.max(0, visibleNoticeCount - 1) * NOTICE_CARD_GAP
              + NOTICE_WINDOW_VERTICAL_PADDING,
          )
          : 1
        if (!hasNotices) return

        if (window.sessionStorage.getItem(NOTICE_WINDOW_MANUAL_KEY) === 'true') return

        const geometry = readPetGeometry()
        if (geometry) {
          const noticeScaleFactor = await noticeWindow.scaleFactor()
          const x = Math.max(8, geometry.centerX - NOTICE_WINDOW_WIDTH / 2)
          const y = Math.max(8, geometry.top - noticeHeight - NOTICE_WINDOW_GAP)
          await noticeWindow.setPosition(new LogicalPosition(x, y).toPhysical(noticeScaleFactor))
        }
      } catch (e) {
        console.error('Failed to position notice window:', e)
      }
    }

    const resizeNoticeWindow = async () => {
      try {
        const noticeWindow = getCurrentWindow()
        const hasNotices = notices.length > 0
        const visibleNoticeCount = Math.max(1, Math.min(notices.length, 2))
        const noticeHeight = hasNotices
          ? Math.min(
            NOTICE_WINDOW_HEIGHT,
            visibleNoticeCount * NOTICE_CARD_HEIGHT
              + Math.max(0, visibleNoticeCount - 1) * NOTICE_CARD_GAP
              + NOTICE_WINDOW_VERTICAL_PADDING,
          )
          : 1
        await noticeWindow.setSize(new LogicalSize(NOTICE_WINDOW_WIDTH, noticeHeight))
        await noticeWindow.setIgnoreCursorEvents(!hasNotices)

        if (hasNotices) {
          await emit('request-pet-geometry')
          if (window.sessionStorage.getItem(NOTICE_WINDOW_MANUAL_KEY) === 'true') return

          const geometry = readPetGeometry()
          if (geometry) {
            const noticeScaleFactor = await noticeWindow.scaleFactor()
            const x = Math.max(8, geometry.centerX - NOTICE_WINDOW_WIDTH / 2)
            const y = Math.max(8, geometry.top - noticeHeight - NOTICE_WINDOW_GAP)
            await noticeWindow.setPosition(new LogicalPosition(x, y).toPhysical(noticeScaleFactor))
          } else {
            const petWindow = await Window.getByLabel('pet')
            if (petWindow) {
            const [petPosition, petSize, petScaleFactor, noticeScaleFactor] = await Promise.all([
              petWindow.outerPosition(),
              petWindow.outerSize(),
              petWindow.scaleFactor(),
              noticeWindow.scaleFactor(),
            ])
            const petLogicalPosition = petPosition.toLogical(petScaleFactor)
            const petLogicalSize = petSize.toLogical(petScaleFactor)
            const petSpriteTop = petLogicalPosition.y + petLogicalSize.height - displayHeight
            const x = Math.max(8, petLogicalPosition.x + (petLogicalSize.width - NOTICE_WINDOW_WIDTH) / 2)
            const y = Math.max(8, petSpriteTop - noticeHeight - NOTICE_WINDOW_GAP)
            await noticeWindow.setPosition(new LogicalPosition(x, y).toPhysical(noticeScaleFactor))
            }
          }
        }
      } catch (e) {
        console.error('Failed to resize notice window:', e)
      }
    }

    resizeNoticeWindow()

    const unlistenGeometry = listenSafe('pet-geometry', (event) => {
      const payload = event.payload
      if (!payload || typeof payload !== 'object') return
      petGeometryRef.current = payload
      positionNoticeWindow()
    })

    return () => {
      cleanupListener(unlistenGeometry)
    }
  }, [displayHeight, notices.length, windowLabel])

  useEffect(() => {
    if (windowLabel !== 'pet') return undefined

    const petWindow = getCurrentWindow()
    let disposed = false
    let ignored = false
    let disabled = false

    const setIgnored = async (nextIgnored) => {
      if (disabled || ignored === nextIgnored) return
      try {
        await petWindow.setIgnoreCursorEvents(nextIgnored)
        if (!disposed) ignored = nextIgnored
      } catch (e) {
        disabled = true
        console.error('Failed to update pet cursor passthrough:', e)
      }
    }

    const updateCursorPassthrough = async () => {
      if (disposed || disabled) return

      if (!hasPetPanels) {
        await setIgnored(false)
        return
      }

      try {
        const [cursor, position, scaleFactor] = await Promise.all([
          cursorPosition(),
          petWindow.outerPosition(),
          petWindow.scaleFactor(),
        ])
        if (disposed) return

        const isInsidePetSurface = cursorLocalPointCandidates(cursor, position, scaleFactor)
          .some(({ x, y }) => isPointInsideVisiblePetSurface(x, y))

        await setIgnored(!isInsidePetSurface)
      } catch (e) {
        console.error('Failed to poll pet cursor passthrough:', e)
        await setIgnored(false)
        disabled = true
      }
    }

    updateCursorPassthrough()
    const timer = window.setInterval(updateCursorPassthrough, POINTER_PASSTHROUGH_POLL_MS)

    return () => {
      disposed = true
      window.clearInterval(timer)
      petWindow.setIgnoreCursorEvents(false).catch(() => {})
    }
  }, [hasPetPanels, windowLabel])

  useEffect(() => {
    usageDashboardPinnedRef.current = usageDashboardPinned
    try {
      window.localStorage.setItem(USAGE_DASHBOARD_PINNED_KEY, String(usageDashboardPinned))
    } catch {
      // The setting is still usable for this session if persistence is unavailable.
    }
  }, [usageDashboardPinned])

  // Determine if this is the settings window
  useEffect(() => {
    const init = async () => {
      let label = ''

      try {
        const currentWindow = getCurrentWindow()
        const rawLabel = currentWindow.label
        label = typeof rawLabel?.then === 'function' ? await rawLabel : rawLabel
      } catch (e) {
        console.error('Failed to read window label:', e)
        setBootError(String(e))
      }

      if (!label) {
        label = globalThis.innerWidth > 420 || globalThis.innerHeight > 360 ? 'settings' : 'pet'
      }

      setWindowLabel(label)

      if (label === 'notices') {
        loadLanguage()
        return
      }

      if (label === 'pet') {
        loadLanguage()
        loadPetScale()
        loadPetList()
        loadInitialPet()
        loadUsageMetrics()
      } else {
        loadPetScale()
        loadPetList()
        loadMessageMap()
        loadWsStatus()
        loadLiveSourcesStatus()
        loadLiveSourcePrefixEnabled()
        loadUserPetDir()
        loadPetLibrary(1)
        loadLanguage()
      }
    }
    init()
  }, [])

  // Listen for state changes from backend
  useEffect(() => {
    const unlistenState = listenSafe('state-changed', (event) => {
      const newState = event.payload
      stateRef.current = newState
      setCurrentState(newState)
      frameRef.current = 0
      setCurrentFrame(0)
      stateStartRef.current = Date.now()
    })

    // Listen for pet-loaded events (e.g. from settings window switching pets)
    const unlistenPetLoaded = listenSafe('pet-loaded', (event) => {
      const config = event.payload
      const source = getSpritesheetSource(config)
      if (source) {
        setSpritesheet(source)
      }
    })

    const unlistenScaleChanged = listenSafe('pet-scale-changed', (event) => {
      setPetScale(event.payload || DEFAULT_PET_SCALE)
    })

    const unlistenCodexBubble = listenSafe('codex-bubble', (event) => {
      const text = event.payload?.text
      if (!text) return

      setBubble({
        text,
        source: event.payload?.source || '',
        sourceLabel: event.payload?.sourceLabel || '',
      })
      if (bubbleTimerRef.current) {
        clearTimeout(bubbleTimerRef.current)
      }
      bubbleTimerRef.current = setTimeout(() => {
        setBubble(null)
        bubbleTimerRef.current = null
      }, 4200)
    })

    const unlistenPetNotice = listenSafe('pet-notice', (event) => {
      if (!event.payload) return

      const notice = normalizeNotice(event.payload)
      setNotices((prev) => {
        const key = notice.groupKey || notice.id
        const withoutExisting = prev.filter((item) => (item.groupKey || item.id) !== key)
        return [notice, ...withoutExisting].slice(0, 8)
      })
    })

    const unlistenPetUsage = listenSafe('pet-usage', (event) => {
      if (!event.payload) return

      const metric = normalizeUsageMetric(event.payload)
      setUsageMetrics((prev) => {
        const withoutExisting = prev.filter((item) => item.id !== metric.id)
        return [metric, ...withoutExisting].slice(0, MAX_USAGE_METRICS)
      })
    })

    return () => {
      cleanupListener(unlistenState)
      cleanupListener(unlistenPetLoaded)
      cleanupListener(unlistenScaleChanged)
      cleanupListener(unlistenCodexBubble)
      cleanupListener(unlistenPetNotice)
      cleanupListener(unlistenPetUsage)
      if (bubbleTimerRef.current) {
        clearTimeout(bubbleTimerRef.current)
      }
      clearUsageDashboardAutoHide()
    }
  }, [])

  useEffect(() => {
    if (!notices.some((notice) => notice.ttlSeconds && notice.ttlSeconds > 0)) return undefined

    const timer = setInterval(() => {
      const now = Date.now()
      setNotices((prev) => (
        prev.filter((notice) => {
          if (!notice.ttlSeconds || notice.ttlSeconds <= 0) return true
          return now - notice.receivedAt < notice.ttlSeconds * 1000
        })
      ))
    }, 1000)

    return () => clearInterval(timer)
  }, [notices])

  // Animation loop
  useEffect(() => {
    if (windowLabel !== 'pet' || !spritesheet) return

    const animate = (timestamp) => {
      const durations = getStateDurations(stateRef.current)
      const frameCount = FRAME_COUNTS[stateRef.current] || 6
      
      if (durations.length > 0) {
        const elapsed = timestamp - lastTimeRef.current
        const currentDuration = durations[frameRef.current % durations.length]
        
        if (elapsed >= currentDuration) {
          frameRef.current = (frameRef.current + 1) % frameCount
          setCurrentFrame(frameRef.current)
          lastTimeRef.current = timestamp
        }
      }

      // Auto-return to idle after state duration
      const stateDuration = getStateDuration(stateRef.current)
      if (Date.now() - stateStartRef.current >= stateDuration) {
        if (stateRef.current !== 'idle') {
          stateRef.current = 'idle'
          setCurrentState('idle')
          frameRef.current = 0
          setCurrentFrame(0)
          stateStartRef.current = Date.now()
        }
      }

      animRef.current = requestAnimationFrame(animate)
    }

    animRef.current = requestAnimationFrame(animate)
    return () => {
      if (animRef.current) {
        cancelAnimationFrame(animRef.current)
      }
    }
  }, [spritesheet, windowLabel])

  const getStateDurations = (state) => {
    const durations = {
      idle: [280, 110, 110, 140, 140, 320],
      'running-right': [120, 120, 120, 120, 120, 120, 120, 220],
      'running-left': [120, 120, 120, 120, 120, 120, 120, 220],
      waving: [140, 140, 140, 280],
      jumping: [140, 140, 140, 140, 280],
      failed: [140, 140, 140, 140, 140, 140, 140, 240],
      waiting: [150, 150, 150, 150, 150, 260],
      running: [120, 120, 120, 120, 120, 220],
      review: [150, 150, 150, 150, 150, 280],
    }
    return durations[state] || [200]
  }

  const getStateDuration = (state) => {
    const durations = {
      waving: 2000,
      jumping: 2000,
      failed: 3000,
      running: 5000,
      'running-right': 5000,
      'running-left': 5000,
      waiting: 10000,
      review: 5000,
      idle: Infinity,
    }
    return durations[state] || 5000
  }

  const getBackgroundPosition = () => {
    const row = STATE_ROWS[currentState] || 0
    const x = -(currentFrame * displayWidth)
    const y = -(row * displayHeight)
    return `${x}px ${y}px`
  }

  const loadPetScale = async () => {
    try {
      const scale = await invoke('get_pet_scale')
      setPetScale(scale)
    } catch (e) {
      console.error('Failed to load pet scale:', e)
    }
  }

  const loadPetList = async () => {
    try {
      const list = await invoke('get_pet_list')
      setPets(list)
    } catch (e) {
      console.error('Failed to load pets:', e)
    }
  }

  const loadPetLibrary = async (page = petLibraryPage.page || 1) => {
    setPetLibraryStatus('loading')
    try {
      const result = await invoke('get_pet_library', { page })
      const items = Array.isArray(result?.items) ? result.items : Array.isArray(result) ? result : []
      setPetLibrary(items)
      setPetLibraryPage({
        page: Number(result?.page) || page || 1,
        pageSize: Number(result?.pageSize || result?.page_size) || items.length || 30,
        total: Number(result?.total) || items.length,
        totalPages: Number(result?.totalPages || result?.total_pages) || 1,
        fromCache: Boolean(result?.fromCache || result?.from_cache),
      })
      setPetLibraryStatus('ready')
    } catch (e) {
      console.error('Failed to load pet library:', e)
      setPetLibraryStatus('error')
    }
  }

  const handleChangePetLibraryPage = (page) => {
    const nextPage = Math.max(1, Math.min(Number(page) || 1, petLibraryPage.totalPages || 1))
    loadPetLibrary(nextPage)
  }

  const loadUserPetDir = async () => {
    try {
      const dir = await invoke('get_user_pet_dir')
      setUserPetDir(dir)
    } catch (e) {
      console.error('Failed to load user pet dir:', e)
    }
  }

  const loadInitialPet = async () => {
    try {
      const list = await invoke('get_pet_list')
      const validPet =
        list.find(p => p.id === 'claude' && p.has_spritesheet) ||
        list.find(p => p.has_spritesheet) ||
        list[0]
      if (validPet) {
        const pet = await invoke('load_pet', { petId: validPet.id })
        setSpritesheet(getSpritesheetSource(pet))
      }
    } catch (e) {
      console.error('Failed to load pet:', e)
    }
  }

  const loadMessageMap = async () => {
    try {
      const map = await invoke('get_message_map')
      setMessageMap(map)
    } catch (e) {
      console.error('Failed to load message map:', e)
    }
  }

  const loadWsStatus = async () => {
    try {
      const status = await invoke('get_websocket_status')
      setWsStatus(status)
    } catch (e) {
      console.error('Failed to load WS status:', e)
    }
  }

  const loadLiveSourcesStatus = async () => {
    try {
      const status = await invoke('get_live_sources_status')
      setLiveSourcesStatus({
        codex: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.codex || {}) },
        claude: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.claude || {}) },
        opencode: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.opencode || {}) },
        openclaw: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.openclaw || {}) },
        hermes: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.hermes || {}) },
        antigravity: { ...EMPTY_LIVE_SOURCE_STATUS, ...(status.antigravity || {}) },
      })
    } catch (e) {
      console.error('Failed to load live source status:', e)
    }
  }

  const loadLiveSourcePrefixEnabled = async () => {
    try {
      const enabled = await invoke('get_live_source_prefix_enabled')
      setLiveSourcePrefixEnabled(Boolean(enabled))
    } catch (e) {
      console.error('Failed to load live source prefix setting:', e)
    }
  }

  const loadUsageMetrics = async () => {
    try {
      const metrics = await invoke('get_usage_metrics')
      if (!Array.isArray(metrics) || metrics.length === 0) return
      setUsageMetrics(metrics.map(normalizeUsageMetric).slice(0, MAX_USAGE_METRICS))
    } catch (e) {
      console.error('Failed to load usage metrics:', e)
    }
  }

  const handleLoadPet = async (petId) => {
    try {
      const pet = await invoke('load_pet', { petId })
      const source = getSpritesheetSource(pet)
      if (source) {
        setSpritesheet(source)
      }
      // Backend emits 'pet-loaded' event to the pet window after loading
      loadPetList() // Refresh list to show active
    } catch (e) {
      console.error('Failed to load pet:', e)
    }
  }

  const handleInstallLibraryPet = async (petId) => {
    if (!petId || installingPetId) return

    setInstallingPetId(petId)
    try {
      await invoke('install_library_pet', { petId })
      await loadPetList()
      await loadPetLibrary(petLibraryPage.page)
      await handleLoadPet(petId)
    } catch (e) {
      console.error('Failed to install pet:', e)
    } finally {
      setInstallingPetId('')
    }
  }

  const handleTrigger = async (messageType) => {
    try {
      const newState = await invoke('trigger_state', { messageType })
      stateRef.current = newState
      setCurrentState(newState)
      frameRef.current = 0
      setCurrentFrame(0)
      stateStartRef.current = Date.now()
    } catch (e) {
      console.error('Failed to trigger state:', e)
    }
  }

  const handleTriggerNotice = async (noticeType = 'all') => {
    try {
      await invoke('trigger_notice', { noticeType })
    } catch (e) {
      console.error('Failed to trigger notice:', e)
    }
  }

  const handleUpdateMessageMap = async (key, value) => {
    const newMap = { ...messageMap, [key]: value }
    setMessageMap(newMap)
    try {
      await invoke('update_message_map', { map: newMap })
    } catch (e) {
      console.error('Failed to update message map:', e)
    }
  }

  const handleToggleWs = async () => {
    try {
      await invoke('toggle_websocket', { enabled: !wsStatus.enabled })
      await loadWsStatus()
    } catch (e) {
      console.error('Failed to toggle websocket:', e)
    }
  }

  const handleToggleLiveSource = async (source) => {
    try {
      await invoke('toggle_live_source', {
        source,
        enabled: !liveSourcesStatus[source].enabled,
      })
      await loadLiveSourcesStatus()
    } catch (e) {
      console.error('Failed to toggle live source:', e)
    }
  }

  const handleChangeLiveSourcePath = (source, path) => {
    setLiveSourcesStatus((prev) => ({
      ...prev,
      [source]: {
        ...prev[source],
        path,
      },
    }))
  }

  const handleChangeLiveSourceFocusTarget = (source, focusTarget) => {
    setLiveSourcesStatus((prev) => ({
      ...prev,
      [source]: {
        ...prev[source],
        focusTarget,
      },
    }))
  }

  const handleSaveLiveSourcePath = async (source) => {
    try {
      await invoke('set_live_source_path', {
        source,
        path: liveSourcesStatus[source].path || '',
      })
      await invoke('set_live_source_focus_target', {
        source,
        target: liveSourcesStatus[source].focusTarget || '',
      })
      await loadLiveSourcesStatus()
    } catch (e) {
      console.error('Failed to save live source settings:', e)
    }
  }

  const handleResetLiveSourcePath = async (source) => {
    try {
      await invoke('set_live_source_path', { source, path: '' })
      await invoke('set_live_source_focus_target', { source, target: '' })
      await loadLiveSourcesStatus()
    } catch (e) {
      console.error('Failed to reset live source settings:', e)
    }
  }

  const handleToggleLiveSourcePrefix = async () => {
    try {
      await invoke('set_live_source_prefix_enabled', { enabled: !liveSourcePrefixEnabled })
      await loadLiveSourcePrefixEnabled()
    } catch (e) {
      console.error('Failed to toggle live source prefix:', e)
    }
  }

  const handleSetPetScale = async (scale) => {
    try {
      const appliedScale = await invoke('set_pet_scale', { scale })
      setPetScale(appliedScale)
    } catch (e) {
      console.error('Failed to set pet scale:', e)
    }
  }

  const loadLanguage = async () => {
    try {
      const lang = await invoke('get_language')
      setLocale(lang || 'en')
    } catch (e) {
      console.error('Failed to load language:', e)
    }
  }

  const handleToggleLanguage = async () => {
    const newLocale = locale === 'en' ? 'zh-CN' : 'en'
    setLocale(newLocale)
    try {
      await invoke('set_language', { language: newLocale })
    } catch (e) {
      console.error('Failed to save language:', e)
    }
  }

  const handlePetMouseDown = async (event) => {
    if (event.button !== 0) return
    document.body.classList.add('pet-pointer-capture')

    try {
      await getCurrentWindow().startDragging()
    } catch (e) {
      console.error('Failed to start window drag:', e)
    } finally {
      window.setTimeout(() => {
        document.body.classList.remove('pet-pointer-capture')
      }, 250)
    }
  }

  const handleBubbleClick = async (event) => {
    event.stopPropagation()
    if (!bubble?.source) return

    try {
      await invoke('focus_live_source', { source: bubble.source })
    } catch (e) {
      console.error('Failed to focus live source:', e)
    }
  }

  const handleNoticeDismiss = (event, notice) => {
    event.stopPropagation()
    const key = notice.groupKey || notice.id
    setNotices((prev) => prev.filter((item) => (item.groupKey || item.id) !== key))
  }

  const handleNoticeAction = async (event, notice) => {
    event.stopPropagation()
    if (!notice?.source) return

    try {
      await invoke('handle_notice_action', {
        source: notice.source,
        actionKind: notice.actionKind || 'focus',
      })
    } catch (e) {
      console.error('Failed to handle notice action:', e)
    }
  }

  const handleToggleUsageDashboard = () => {
    if (usageDashboardVisible) {
      setUsageDashboardPinned(false)
      usageDashboardPinnedRef.current = false
      setUsageDashboardTemporaryVisible(false)
      clearUsageDashboardAutoHide()
      return
    }

    showUsageDashboardTemporarily()
  }

  const handleToggleUsageDashboardPinned = () => {
    const nextPinned = !usageDashboardPinnedRef.current
    usageDashboardPinnedRef.current = nextPinned
    setUsageDashboardPinned(nextPinned)

    if (nextPinned) {
      clearUsageDashboardAutoHide()
      setUsageDashboardTemporaryVisible(false)
    } else {
      showUsageDashboardTemporarily()
    }
  }

  const handleOpenSettings = async () => {
    try {
      await invoke('open_settings_window')
    } catch (e) {
      console.error('Failed to open settings window:', e)
    }
  }

  if (!windowLabel) {
    return (
      <div className="settings-container">
        <div className="empty-state">{bootError || t('loading')}</div>
      </div>
    )
  }

  if (windowLabel === 'notices') {
    return (
      <PetWindow
        bubble={null}
        currentState={currentState}
        currentFrame={currentFrame}
        displayHeight={displayHeight}
        displayWidth={displayWidth}
        getBackgroundPosition={getBackgroundPosition}
        handleBubbleClick={handleBubbleClick}
        handleNoticeAction={handleNoticeAction}
        handleNoticeDismiss={handleNoticeDismiss}
        handleOpenSettings={handleOpenSettings}
        handlePetMouseDown={handlePetMouseDown}
        handleTrigger={handleTrigger}
        handleToggleUsageDashboard={handleToggleUsageDashboard}
        handleToggleUsageDashboardPinned={handleToggleUsageDashboardPinned}
        handleUsageDashboardActivity={showUsageDashboardTemporarily}
        notices={notices}
        noticeOnly
        petScale={petScale}
        spritesheet=""
        t={t}
        usageDashboardPinned={usageDashboardPinned}
        usageDashboardVisible={false}
        usageMetrics={[]}
        windowHeight={Math.min(
          NOTICE_WINDOW_HEIGHT,
          Math.max(1, Math.min(notices.length || 1, 2)) * NOTICE_CARD_HEIGHT
            + Math.max(0, Math.min(notices.length || 1, 2) - 1) * NOTICE_CARD_GAP
            + NOTICE_WINDOW_VERTICAL_PADDING,
        )}
        windowWidth={NOTICE_WINDOW_WIDTH}
      />
    )
  }

  if (windowLabel !== 'settings') {
    return (
      <PetWindow
        bubble={bubble}
        currentState={currentState}
        currentFrame={currentFrame}
        displayHeight={displayHeight}
        displayWidth={displayWidth}
        getBackgroundPosition={getBackgroundPosition}
        handleBubbleClick={handleBubbleClick}
        handleNoticeAction={handleNoticeAction}
        handleNoticeDismiss={handleNoticeDismiss}
        handleOpenSettings={handleOpenSettings}
        handlePetMouseDown={handlePetMouseDown}
        handleTrigger={handleTrigger}
        handleToggleUsageDashboard={handleToggleUsageDashboard}
        handleToggleUsageDashboardPinned={handleToggleUsageDashboardPinned}
        handleUsageDashboardActivity={showUsageDashboardTemporarily}
        notices={[]}
        petScale={petScale}
        spritesheet={spritesheet}
        t={t}
        usageDashboardPinned={usageDashboardPinned}
        usageDashboardVisible={usageDashboardVisible}
        usageMetrics={visibleUsageMetrics}
        windowHeight={windowHeight}
        windowWidth={windowWidth}
      />
    )
  }

  return (
    <SettingsWindow
      handleChangeLiveSourceFocusTarget={handleChangeLiveSourceFocusTarget}
      handleChangeLiveSourcePath={handleChangeLiveSourcePath}
      handleLoadPet={handleLoadPet}
      handleInstallLibraryPet={handleInstallLibraryPet}
      handleChangePetLibraryPage={handleChangePetLibraryPage}
      handleRefreshPetLibrary={loadPetLibrary}
      handleResetLiveSourcePath={handleResetLiveSourcePath}
      handleSaveLiveSourcePath={handleSaveLiveSourcePath}
      handleSetPetScale={handleSetPetScale}
      handleToggleLanguage={handleToggleLanguage}
      handleToggleLiveSource={handleToggleLiveSource}
      handleToggleLiveSourcePrefix={handleToggleLiveSourcePrefix}
      handleToggleWs={handleToggleWs}
      handleTrigger={handleTrigger}
      handleTriggerNotice={handleTriggerNotice}
      handleUpdateMessageMap={handleUpdateMessageMap}
      liveSourcePrefixEnabled={liveSourcePrefixEnabled}
      liveSourcesStatus={liveSourcesStatus}
      locale={locale}
      messageMap={messageMap}
      pets={pets}
      petLibrary={petLibrary}
      petLibraryPage={petLibraryPage}
      petLibraryStatus={petLibraryStatus}
      petScale={petScale}
      installingPetId={installingPetId}
      t={t}
      userPetDir={userPetDir}
      wsStatus={wsStatus}
    />
  )
}

export default App
