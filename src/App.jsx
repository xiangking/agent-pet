import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { t as translate } from './locales'

// Codex pet protocol constants
const CELL_WIDTH = 192
const CELL_HEIGHT = 208
const ATLAS_WIDTH = 1536
const ATLAS_HEIGHT = 1872
const MIN_WINDOW_WIDTH = 240
const BUBBLE_SPACE_HEIGHT = 92
const CODEX_ORIGINAL_SCALE = 0.45
const DEFAULT_PET_SCALE = CODEX_ORIGINAL_SCALE
const PET_SCALE_OPTIONS = [
  { labelKey: 'sizeSmall', value: CODEX_ORIGINAL_SCALE * 0.75, percent: 75 },
  { labelKey: 'sizeOriginal', value: CODEX_ORIGINAL_SCALE, percent: 100 },
  { labelKey: 'sizeLarge', value: CODEX_ORIGINAL_SCALE * 1.25, percent: 125 },
  { labelKey: 'sizeXl', value: CODEX_ORIGINAL_SCALE * 1.5, percent: 150 },
]
const LIVE_SOURCES = [
  { key: 'codex', label: 'Codex' },
  { key: 'claude', label: 'Claude Code' },
  { key: 'opencode', label: 'opencode' },
  { key: 'openclaw', label: 'OpenClaw' },
  { key: 'hermes', label: 'Hermes Agent' },
]
const EMPTY_LIVE_SOURCE_STATUS = {
  enabled: true,
  path: '',
  defaultPath: '',
  focusTarget: '',
  defaultFocusTarget: '',
}
const STATE_ROWS = {
  idle: 0,
  'running-right': 1,
  'running-left': 2,
  waving: 3,
  jumping: 4,
  failed: 5,
  waiting: 6,
  running: 7,
  review: 8,
}

const FRAME_COUNTS = {
  idle: 6,
  'running-right': 8,
  'running-left': 8,
  waving: 4,
  jumping: 5,
  failed: 8,
  waiting: 6,
  running: 6,
  review: 6,
}

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

  // Pet window
  if (!isSettings) {
    return (
      <div 
        className="pet-container"
        data-tauri-drag-region
        style={{ width: windowWidth, height: windowHeight, '--pet-display-height': `${displayHeight}px` }}
        onMouseDown={handlePetMouseDown}
        onClick={() => handleTrigger('jumping')}
      >
        {bubble?.text && (
          <div
            className={`pet-bubble ${bubble.source ? 'clickable' : ''}`}
            onMouseDown={(event) => event.stopPropagation()}
            onClick={handleBubbleClick}
            title={bubble.sourceLabel || bubble.source}
          >
            {bubble.text}
          </div>
        )}
        {spritesheet ? (
          <div
            className="pet-sprite"
            style={{
              backgroundImage: `url(${spritesheet})`,
              backgroundPosition: getBackgroundPosition(),
              backgroundSize: `${ATLAS_WIDTH * petScale}px ${ATLAS_HEIGHT * petScale}px`,
              width: displayWidth,
              height: displayHeight,
            }}
          />
        ) : (
          <div style={{ 
            width: displayWidth, 
            height: displayHeight, 
            display: 'flex', 
            alignItems: 'center', 
            justifyContent: 'center',
            color: '#666',
            fontSize: 12,
          }}>
            {t('noLoaded')}
          </div>
        )}
      </div>
    )
  }

  // Settings window
  return (
    <div className="settings-container">
      <div className="settings-shell">
        <header className="settings-header">
          <div className="settings-title-row">
            <h1>{t('title')}</h1>
            <button className="button button-ghost language-toggle" onClick={handleToggleLanguage}>
              {locale === 'en' ? '中文' : 'EN'}
            </button>
          </div>
        </header>

        <main className="settings-grid">
          <section className="settings-section pets-section">
            <div className="section-heading">
              <h2>{t('pets')}</h2>
              <span className="count-pill">{pets.length}</span>
            </div>
            <div className="pet-list">
              {pets.map((pet) => (
                <button
                  key={pet.id}
                  type="button"
                  className={`pet-item ${pet.has_spritesheet ? '' : 'disabled'}`}
                  onClick={() => pet.has_spritesheet && handleLoadPet(pet.id)}
                >
                  <span className="pet-avatar">{pet.display_name.slice(0, 1)}</span>
                  <span className="pet-item-info">
                    <span className="pet-item-name">{pet.display_name}</span>
                    <span className="pet-item-desc">{pet.description}</span>
                  </span>
                  {!pet.has_spritesheet && (
                    <span className="status-badge disconnected">{t('missingSprite')}</span>
                  )}
                </button>
              ))}
              {pets.length === 0 && (
                <div className="empty-state">
                  {t('noPets', { dir: userPetDir || t('userPetDirectory') })}
                </div>
              )}
            </div>
          </section>

          <section className="settings-section">
            <div className="section-heading">
              <h2>{t('wsServer')}</h2>
              <span className={`status-badge ${wsStatus.enabled ? 'connected' : 'disconnected'}`}>
                {wsStatus.enabled ? t('enabled') : t('disabled')}
              </span>
            </div>
            <div className="metric-row">
              <span>{t('port')}</span>
              <strong>{wsStatus.port}</strong>
            </div>
            <button className="button button-primary" onClick={handleToggleWs}>
              {wsStatus.enabled ? t('disable') : t('enable')}
            </button>
            <div className="helper-text">
              {t('connectTo', { port: wsStatus.port })}
            </div>
          </section>

          <section className="settings-section">
            <div className="section-heading">
              <h2>{t('petSize')}</h2>
            </div>
            <div className="segmented-options">
              {PET_SCALE_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={`segment-button ${Math.abs(petScale - option.value) < 0.01 ? 'selected' : ''}`}
                  onClick={() => handleSetPetScale(option.value)}
                >
                  <span>{t(option.labelKey)}</span>
                  <strong>{option.percent}%</strong>
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section live-section">
            <div className="section-heading">
              <h2>{t('liveSources')}</h2>
              <div className="heading-actions">
                <span className={`status-badge ${liveSourcePrefixEnabled ? 'connected' : 'disconnected'}`}>
                  {liveSourcePrefixEnabled ? t('prefixOn') : t('prefixOff')}
                </span>
                <button className="button button-ghost" onClick={handleToggleLiveSourcePrefix}>
                  {liveSourcePrefixEnabled ? t('hidePrefix') : t('showPrefix')}
                </button>
              </div>
            </div>
            <div className="live-sources">
              {LIVE_SOURCES.map((source) => {
                const status = liveSourcesStatus[source.key]
                const enabled = Boolean(status.enabled)
                const defaultPath = status.defaultPath || ''
                const path = status.path || defaultPath
                const defaultFocusTarget = status.defaultFocusTarget || ''
                const focusTarget = status.focusTarget || defaultFocusTarget

                return (
                  <div key={source.key} className="live-source-item">
                    <div className="live-source-main">
                      <div className="live-source-name">{source.label}</div>
                      <div className="live-source-path">{t('defaultLabel')} {defaultPath || t('loading')}</div>
                    </div>
                    <div className="live-source-path-field field-stack">
                      <label className="field-label">{t('defaultLabel')}</label>
                      <input
                        type="text"
                        value={path}
                        onChange={(e) => handleChangeLiveSourcePath(source.key, e.target.value)}
                      />
                      <label className="field-label">{t('focusTarget')}</label>
                      <input
                        type="text"
                        value={focusTarget}
                        placeholder={t('focusTargetHint')}
                        onChange={(e) => handleChangeLiveSourceFocusTarget(source.key, e.target.value)}
                      />
                    </div>
                    <div className="live-source-actions">
                      <span className={`status-badge ${enabled ? 'connected' : 'disconnected'}`}>
                        {enabled ? t('enabled') : t('disabled')}
                      </span>
                      <button className="button button-ghost" onClick={() => handleToggleLiveSource(source.key)}>
                        {enabled ? t('disable') : t('enable')}
                      </button>
                      <button className="button button-ghost" onClick={() => handleResetLiveSourcePath(source.key)}>
                        {t('reset')}
                      </button>
                      <button className="button button-primary" onClick={() => handleSaveLiveSourcePath(source.key)}>
                        {t('save')}
                      </button>
                    </div>
                  </div>
                )
              })}
            </div>
          </section>

          <section className="settings-section message-section">
            <div className="section-heading">
              <h2>{t('messageMapping')}</h2>
            </div>
            <div className="message-map">
              {Object.entries(messageMap).map(([msgType, state]) => (
                <div key={msgType} className="message-map-item">
                  <label>{msgType}</label>
                  <select
                    value={state}
                    onChange={(e) => handleUpdateMessageMap(msgType, e.target.value)}
                  >
                    {Object.keys(STATE_ROWS).map((s) => (
                      <option key={s} value={s}>{s}</option>
                    ))}
                  </select>
                </div>
              ))}
            </div>
          </section>

          <section className="settings-section test-section">
            <div className="section-heading">
              <h2>{t('test')}</h2>
            </div>
            <div className="test-actions">
              {Object.keys(STATE_ROWS).map((state) => (
                <button className="button button-ghost" key={state} onClick={() => handleTrigger(state)}>
                  {state}
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section directory-section">
            <div className="section-heading">
              <h2>{t('petDirectory')}</h2>
            </div>
            <div className="directory-row">
              <strong>{t('userPets')}</strong>
              <code>{userPetDir || t('loading')}</code>
            </div>
            <div className="directory-row">
              <strong>{t('builtInPets')}</strong>
              <code>{`{project}/pets/`}</code>
            </div>
            <div className="helper-text">
              {t('petFolderHint').split('\n').map((line) => (
                <span key={line}>
                  {line}
                  <br />
                </span>
              ))}
            </div>
          </section>
        </main>
      </div>
    </div>
  )
}

export default App
