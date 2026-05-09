export const CELL_WIDTH = 192
export const CELL_HEIGHT = 208
export const ATLAS_WIDTH = 1536
export const ATLAS_HEIGHT = 1872
export const MIN_WINDOW_WIDTH = 240
export const BUBBLE_SPACE_HEIGHT = 92
export const CODEX_ORIGINAL_SCALE = 0.45
export const DEFAULT_PET_SCALE = CODEX_ORIGINAL_SCALE

export const PET_SCALE_OPTIONS = [
  { labelKey: 'sizeSmall', value: CODEX_ORIGINAL_SCALE * 0.75, percent: 75 },
  { labelKey: 'sizeOriginal', value: CODEX_ORIGINAL_SCALE, percent: 100 },
  { labelKey: 'sizeLarge', value: CODEX_ORIGINAL_SCALE * 1.25, percent: 125 },
  { labelKey: 'sizeXl', value: CODEX_ORIGINAL_SCALE * 1.5, percent: 150 },
]

export const LIVE_SOURCES = [
  { key: 'codex', label: 'Codex' },
  { key: 'claude', label: 'Claude Code' },
  { key: 'opencode', label: 'opencode' },
  { key: 'openclaw', label: 'OpenClaw' },
  { key: 'hermes', label: 'Hermes Agent' },
  { key: 'antigravity', label: 'Antigravity' },
]

export const EMPTY_LIVE_SOURCE_STATUS = {
  enabled: true,
  path: '',
  defaultPath: '',
  focusTarget: '',
  defaultFocusTarget: '',
}

export const STATE_ROWS = {
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

export const FRAME_COUNTS = {
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
