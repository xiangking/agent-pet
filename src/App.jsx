import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
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
  STATE_ROWS,
} from './constants'
const getSpritesheetSource = (petConfig) => {
  return petConfig?.spritesheetDataUrl || petConfig?.spritesheet_data_url || ''
}

function App() {
  const [currentState, setCurrentState] = useState('idle')
  const [currentFrame, setCurrentFrame] = useState(0)
  const [spritesheet, setSpritesheet] = useState('')
  const [isSettings, setIsSettings] = useState(false)
  const [pets, setPets] = useState([])
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
  const [userPetDir, setUserPetDir] = useState('')
  const [locale, setLocale] = useState('en')

  const t = (key, params) => translate(locale, key, params)
  
  const animRef = useRef(null)
  const bubbleTimerRef = useRef(null)
  const lastTimeRef = useRef(0)
  const frameRef = useRef(0)
  const stateRef = useRef('idle')
  const stateStartRef = useRef(Date.now())
  const displayWidth = CELL_WIDTH * petScale
  const displayHeight = CELL_HEIGHT * petScale
  const windowWidth = Math.max(displayWidth, MIN_WINDOW_WIDTH)
  const windowHeight = displayHeight + BUBBLE_SPACE_HEIGHT

  // Determine if this is the settings window
  useEffect(() => {
    const init = async () => {
      const window = getCurrentWebviewWindow()
      const label = await window.label
      setIsSettings(label === 'settings')
      
      if (label === 'pet') {
        loadLanguage()
        loadPetScale()
        loadPetList()
        loadInitialPet()
      } else {
        loadPetScale()
        loadPetList()
        loadMessageMap()
        loadWsStatus()
        loadLiveSourcesStatus()
        loadLiveSourcePrefixEnabled()
        loadUserPetDir()
        loadLanguage()
      }
    }
    init()
  }, [])

  // Listen for state changes from backend
  useEffect(() => {
    const unlistenState = listen('state-changed', (event) => {
      const newState = event.payload
      stateRef.current = newState
      setCurrentState(newState)
      frameRef.current = 0
      setCurrentFrame(0)
      stateStartRef.current = Date.now()
    })

    // Listen for pet-loaded events (e.g. from settings window switching pets)
    const unlistenPetLoaded = listen('pet-loaded', (event) => {
      const config = event.payload
      const source = getSpritesheetSource(config)
      if (source) {
        setSpritesheet(source)
      }
    })

    const unlistenScaleChanged = listen('pet-scale-changed', (event) => {
      setPetScale(event.payload || DEFAULT_PET_SCALE)
    })

    const unlistenCodexBubble = listen('codex-bubble', (event) => {
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

    return () => {
      unlistenState.then(f => f())
      unlistenPetLoaded.then(f => f())
      unlistenScaleChanged.then(f => f())
      unlistenCodexBubble.then(f => f())
      if (bubbleTimerRef.current) {
        clearTimeout(bubbleTimerRef.current)
      }
    }
  }, [])

  // Animation loop
  useEffect(() => {
    if (isSettings || !spritesheet) return

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
  }, [spritesheet, isSettings])

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

    try {
      await getCurrentWindow().startDragging()
    } catch (e) {
      console.error('Failed to start window drag:', e)
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

  if (!isSettings) {
    return (
      <PetWindow
        bubble={bubble}
        currentState={currentState}
        currentFrame={currentFrame}
        displayHeight={displayHeight}
        displayWidth={displayWidth}
        getBackgroundPosition={getBackgroundPosition}
        handleBubbleClick={handleBubbleClick}
        handlePetMouseDown={handlePetMouseDown}
        handleTrigger={handleTrigger}
        petScale={petScale}
        spritesheet={spritesheet}
        t={t}
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
      handleResetLiveSourcePath={handleResetLiveSourcePath}
      handleSaveLiveSourcePath={handleSaveLiveSourcePath}
      handleSetPetScale={handleSetPetScale}
      handleToggleLanguage={handleToggleLanguage}
      handleToggleLiveSource={handleToggleLiveSource}
      handleToggleLiveSourcePrefix={handleToggleLiveSourcePrefix}
      handleToggleWs={handleToggleWs}
      handleTrigger={handleTrigger}
      handleUpdateMessageMap={handleUpdateMessageMap}
      liveSourcePrefixEnabled={liveSourcePrefixEnabled}
      liveSourcesStatus={liveSourcesStatus}
      locale={locale}
      messageMap={messageMap}
      pets={pets}
      petScale={petScale}
      t={t}
      userPetDir={userPetDir}
      wsStatus={wsStatus}
    />
  )
}

export default App
