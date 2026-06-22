import { useEffect, useState } from 'react'
import { LIVE_SOURCES, PET_SCALE_OPTIONS, STATE_ROWS } from './constants'

const INSTALLED_PETS_PAGE_SIZE = 5
const NOTICE_TEST_TYPES = [
  ['approval_required', 'testNoticeApproval'],
  ['confirm_required', 'testNoticeConfirm'],
  ['press_enter_required', 'testNoticeEnter'],
  ['context_compacting', 'testNoticeContext'],
  ['task_failed', 'testNoticeFailed'],
  ['info', 'testNoticeInfo'],
]

function SettingsWindow({
  handleChangeLiveSourceFocusTarget,
  handleChangeLiveSourcePath,
  handleLoadPet,
  handleInstallLibraryPet,
  handleChangePetLibraryPage,
  handleRefreshPetLibrary,
  handleResetLiveSourcePath,
  handleSaveLiveSourcePath,
  handleSetPetScale,
  handleToggleLanguage,
  handleToggleLiveSource,
  handleToggleLiveSourcePrefix,
  handleToggleWs,
  handleTrigger,
  handleTriggerNotice,
  handleUpdateMessageMap,
  liveSourcePrefixEnabled,
  liveSourcesStatus,
  locale,
  messageMap,
  pets,
  petLibrary,
  petLibraryPage,
  petLibraryStatus,
  petScale,
  installingPetId,
  t,
  userPetDir,
  wsStatus,
}) {
  const asText = (value, fallback = '') => {
    if (typeof value === 'string') return value
    if (value === null || value === undefined) return fallback
    return String(value)
  }
  const text = (key, params) => {
    try {
      return asText(typeof t === 'function' ? t(key, params) : key, key)
    } catch {
      return key
    }
  }
  const installedPets = Array.isArray(pets) ? pets : []
  const libraryPets = Array.isArray(petLibrary) ? petLibrary : []
  const [installedPetPage, setInstalledPetPage] = useState(1)
  const totalInstalledPages = Math.max(1, Math.ceil(installedPets.length / INSTALLED_PETS_PAGE_SIZE))
  const currentInstalledPage = Math.min(installedPetPage, totalInstalledPages)
  const installedPageStart = (currentInstalledPage - 1) * INSTALLED_PETS_PAGE_SIZE
  const visibleInstalledPets = installedPets.slice(
    installedPageStart,
    installedPageStart + INSTALLED_PETS_PAGE_SIZE,
  )
  const isFirstInstalledPage = currentInstalledPage <= 1
  const isLastInstalledPage = currentInstalledPage >= totalInstalledPages
  const libraryPage = petLibraryPage && typeof petLibraryPage === 'object' ? petLibraryPage : {}
  const currentLibraryPage = Number(libraryPage.page) || 1
  const totalLibraryPages = Math.max(1, Number(libraryPage.totalPages || libraryPage.total_pages) || 1)
  const totalLibraryPets = Number(libraryPage.total) || libraryPets.length
  const isFirstLibraryPage = currentLibraryPage <= 1
  const isLastLibraryPage = currentLibraryPage >= totalLibraryPages
  const sourceStatuses = liveSourcesStatus && typeof liveSourcesStatus === 'object' ? liveSourcesStatus : {}
  const mappings = messageMap && typeof messageMap === 'object' ? messageMap : {}
  const installedPetIds = new Set(installedPets.map((pet) => pet.id))
  const builtinPetDescriptions = {
    seedy: {
      en: 'Small green shoots for new ideas.',
      'zh-CN': '为新想法冒出小小绿芽。',
    },
    datawhale: {
      en: 'A tiny blue whale companion for calm focus.',
      'zh-CN': '一只适合安静专注的小蓝鲸伙伴。',
    },
    'null-signal': {
      en: 'A quiet signal from the void.',
      'zh-CN': '来自虚空的一点安静信号。',
    },
    rocky: {
      en: 'A steady little rock when the diff gets large.',
      'zh-CN': '当改动变大时，稳稳陪着你的小石头。',
    },
    claude: {
      en: 'A tiny orange pixel companion with many cute expressions.',
      'zh-CN': '一只橙色像素小伙伴，表情和动作都很可爱。',
    },
    codex: {
      en: 'The original coding companion.',
      'zh-CN': '最初的编码陪伴精灵。',
    },
    bsod: {
      en: 'A tiny blue-screen buddy for debugging days.',
      'zh-CN': '适合调试日的小小蓝屏伙伴。',
    },
    stacky: {
      en: 'A balanced stack for deep work.',
      'zh-CN': '为深度工作保持平衡的小堆叠。',
    },
    fireball: {
      en: 'Hot-path energy for fast iteration.',
      'zh-CN': '为快速迭代补上一点火力。',
    },
    dewey: {
      en: 'A tidy duck for calmer workspace days.',
      'zh-CN': '一只让工作区更平静整洁的小鸭。',
    },
  }
  const categoryLabels = {
    animal: { en: 'Animal', 'zh-CN': '动物' },
    person: { en: 'Character', 'zh-CN': '角色' },
    object: { en: 'Object', 'zh-CN': '物件' },
    creature: { en: 'Creature', 'zh-CN': '生物' },
    Animals: { en: 'Animals', 'zh-CN': '动物' },
    'Anime Characters': { en: 'Anime Characters', 'zh-CN': '动漫角色' },
    'Original Characters': { en: 'Original Characters', 'zh-CN': '原创角色' },
    Robots: { en: 'Robots', 'zh-CN': '机器人' },
  }
  const localizedPetDescription = (pet) => {
    const id = asText(pet.id)
    const dict = builtinPetDescriptions[id]
    if (dict) return dict[locale] || dict.en
    return asText(pet.description)
  }
  const localizedLibraryDescription = (pet) => {
    const category = asText(pet.category)
    const categoryLabel = categoryLabels[category]?.[locale] || category
    const author = asText(pet.author)
    if (locale === 'zh-CN') {
      if (categoryLabel && author) return `${categoryLabel}精灵，由 ${author} 创作。`
      if (categoryLabel) return `${categoryLabel}精灵，可下载后使用。`
      return '可下载后使用的桌面精灵。'
    }
    return asText(pet.description) || (
      categoryLabel
        ? `A downloadable desktop pet in the ${categoryLabel} collection.`
        : 'A downloadable desktop pet.'
    )
  }
  const petThumbStyle = (src) => src
    ? {
        backgroundImage: `url(${src})`,
        backgroundSize: '34px 36.83px',
        backgroundPosition: 'top left',
        backgroundRepeat: 'no-repeat',
      }
    : undefined
  const fallbackInitial = (pet) => asText(pet.display_name || pet.displayName, '?').slice(0, 1)

  useEffect(() => {
    if (installedPetPage > totalInstalledPages) {
      setInstalledPetPage(totalInstalledPages)
    }
  }, [installedPetPage, totalInstalledPages])

  const changeInstalledPetPage = (page) => {
    setInstalledPetPage(Math.max(1, Math.min(Number(page) || 1, totalInstalledPages)))
  }

  return (
    <div className="settings-container">
      <div className="settings-shell">
        <header className="settings-header">
          <div className="settings-title-row">
            <h1>{text('title')}</h1>
            <button className="button button-ghost language-toggle" onClick={handleToggleLanguage}>
              {locale === 'en' ? '中文' : 'EN'}
            </button>
          </div>
        </header>

        <main className="settings-grid">
          <section className="settings-section pets-section">
            <div className="section-heading">
              <div>
                <h2>{text('pets')}</h2>
                <div className="section-subtitle">{text('petLibraryHint')}</div>
              </div>
              <div className="heading-actions">
                <button className="button button-ghost" onClick={() => handleRefreshPetLibrary(currentLibraryPage)}>
                  {text('refresh')}
                </button>
                <span className="count-pill">{text('petCounts', { installed: installedPets.length, total: totalLibraryPets })}</span>
              </div>
            </div>
            <div className="pet-library-layout">
              <div className="pet-column">
                <div className="pet-column-title library-title-row">
                  <span>{text('installedPets')}</span>
                  <span className="pet-page-count">
                    {text('petLibraryPageInfo', { page: currentInstalledPage, totalPages: totalInstalledPages })}
                  </span>
                </div>
                <div className="pet-column-body">
                  <div className="pet-list installed-list">
                    {visibleInstalledPets.map((pet) => (
                      <button
                        key={pet.id}
                        type="button"
                        className={`pet-item ${pet.has_spritesheet ? '' : 'disabled'}`}
                        onClick={() => pet.has_spritesheet && handleLoadPet(pet.id)}
                      >
                        <span
                          className={`pet-avatar pet-thumbnail ${pet.thumbnailDataUrl || pet.thumbnail_data_url ? 'has-image' : ''}`}
                          style={petThumbStyle(pet.thumbnailDataUrl || pet.thumbnail_data_url)}
                        >
                          {!(pet.thumbnailDataUrl || pet.thumbnail_data_url) && fallbackInitial(pet)}
                        </span>
                        <span className="pet-item-info">
                          <span className="pet-item-name">{asText(pet.display_name, pet.id)}</span>
                          <span className="pet-item-desc">{localizedPetDescription(pet)}</span>
                        </span>
                        {!pet.has_spritesheet && (
                          <span className="status-badge disconnected">{text('missingSprite')}</span>
                        )}
                      </button>
                    ))}
                    {installedPets.length === 0 && (
                      <div className="empty-state">
                        {text('noPets', { dir: userPetDir || text('userPetDirectory') })}
                      </div>
                    )}
                  </div>
                  <div className="library-pagination">
                    <button
                      type="button"
                      className="button button-ghost"
                      disabled={isFirstInstalledPage}
                      onClick={() => changeInstalledPetPage(currentInstalledPage - 1)}
                    >
                      {text('previousPage')}
                    </button>
                    <span className="pet-page-count">
                      {text('petLibraryPageInfo', { page: currentInstalledPage, totalPages: totalInstalledPages })}
                    </span>
                    <button
                      type="button"
                      className="button button-ghost"
                      disabled={isLastInstalledPage}
                      onClick={() => changeInstalledPetPage(currentInstalledPage + 1)}
                    >
                      {text('nextPage')}
                    </button>
                  </div>
                </div>
              </div>

              <div className="pet-column">
                <div className="pet-column-title library-title-row">
                  <span>{text('onlinePetLibrary')}</span>
                  <span className="pet-page-count">
                    {text('petLibraryPageInfo', { page: currentLibraryPage, totalPages: totalLibraryPages })}
                    {libraryPage.fromCache && ` · ${text('cached')}`}
                  </span>
                </div>
                <div className="pet-column-body">
                  {petLibraryStatus === 'loading' && (
                    <div className="empty-state pet-list-placeholder">{text('loading')}</div>
                  )}
                  {petLibraryStatus === 'error' && (
                    <div className="empty-state pet-list-placeholder">
                      {text('petLibraryError')}
                    </div>
                  )}
                  {petLibraryStatus !== 'loading' && petLibraryStatus !== 'error' && (
                    <>
                      <div className="pet-list library-list">
                        {libraryPets.map((pet) => {
                          const installed = pet.installed || installedPetIds.has(pet.id)
                          const installing = installingPetId === pet.id

                          return (
                            <div key={pet.id} className="pet-item library-pet-item">
                              <span className={`pet-avatar library-avatar pet-thumbnail ${pet.thumbnailUrl ? 'has-image' : ''}`}>
                                {pet.thumbnailUrl ? (
                                  <img src={pet.thumbnailUrl} alt="" loading="lazy" draggable="false" />
                                ) : fallbackInitial(pet)}
                              </span>
                              <span className="pet-item-info">
                                <span className="pet-item-name">{asText(pet.display_name || pet.displayName, pet.id)}</span>
                                <span className="pet-meta">
                                  {[asText(pet.category), asText(pet.author)].filter(Boolean).join(' · ')}
                                </span>
                                <span className="pet-item-desc">{localizedLibraryDescription(pet)}</span>
                                {pet.license && <span className="pet-license">{asText(pet.license)}</span>}
                              </span>
                              <button
                                type="button"
                                className={`button ${installed ? 'button-ghost' : 'button-primary'} pet-action-button`}
                                disabled={installing || Boolean(installingPetId)}
                                onClick={() => installed ? handleLoadPet(pet.id) : handleInstallLibraryPet(pet.id)}
                              >
                                {installing ? text('downloading') : installed ? text('usePet') : text('downloadPet')}
                              </button>
                            </div>
                          )
                        })}
                        {libraryPets.length === 0 && (
                          <div className="empty-state">{text('emptyPetLibrary')}</div>
                        )}
                      </div>
                      <div className="library-pagination">
                        <button
                          type="button"
                          className="button button-ghost"
                          disabled={isFirstLibraryPage || petLibraryStatus === 'loading'}
                          onClick={() => handleChangePetLibraryPage(currentLibraryPage - 1)}
                        >
                          {text('previousPage')}
                        </button>
                        <span className="pet-page-count">
                          {text('petLibraryPageInfo', { page: currentLibraryPage, totalPages: totalLibraryPages })}
                        </span>
                        <button
                          type="button"
                          className="button button-ghost"
                          disabled={isLastLibraryPage || petLibraryStatus === 'loading'}
                          onClick={() => handleChangePetLibraryPage(currentLibraryPage + 1)}
                        >
                          {text('nextPage')}
                        </button>
                      </div>
                    </>
                  )}
                </div>
              </div>
            </div>
          </section>

          <section className="settings-section">
            <div className="section-heading">
              <h2>{text('wsServer')}</h2>
              <span className={`status-badge ${wsStatus.enabled ? 'connected' : 'disconnected'}`}>
                {wsStatus.enabled ? text('enabled') : text('disabled')}
              </span>
            </div>
            <div className="metric-row">
              <span>{text('port')}</span>
              <strong>{wsStatus.port}</strong>
            </div>
            <button className="button button-primary" onClick={handleToggleWs}>
              {wsStatus.enabled ? text('disable') : text('enable')}
            </button>
            <div className="helper-text">
              {text('connectTo', { port: wsStatus.port })}
            </div>
          </section>

          <section className="settings-section">
            <div className="section-heading">
              <h2>{text('petSize')}</h2>
            </div>
            <div className="segmented-options">
              {PET_SCALE_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={`segment-button ${Math.abs(petScale - option.value) < 0.01 ? 'selected' : ''}`}
                  onClick={() => handleSetPetScale(option.value)}
                >
                  <span>{text(option.labelKey)}</span>
                  <strong>{option.percent}%</strong>
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section live-section">
            <div className="section-heading">
              <h2>{text('liveSources')}</h2>
              <div className="heading-actions">
                <span className={`status-badge ${liveSourcePrefixEnabled ? 'connected' : 'disconnected'}`}>
                  {liveSourcePrefixEnabled ? text('prefixOn') : text('prefixOff')}
                </span>
                <button className="button button-ghost" onClick={handleToggleLiveSourcePrefix}>
                  {liveSourcePrefixEnabled ? text('hidePrefix') : text('showPrefix')}
                </button>
              </div>
            </div>
            <div className="live-sources">
              {LIVE_SOURCES.map((source) => {
                const status = sourceStatuses[source.key] || {}
                const enabled = Boolean(status.enabled)
                const defaultPath = asText(status.defaultPath)
                const path = asText(status.path) || defaultPath
                const defaultFocusTarget = asText(status.defaultFocusTarget)
                const focusTarget = asText(status.focusTarget) || defaultFocusTarget

                return (
                  <div key={source.key} className="live-source-item">
                    <div className="live-source-main">
                      <div className="live-source-name">{asText(source.label, source.key)}</div>
                      <div className="live-source-path">{text('defaultLabel')} {defaultPath || text('loading')}</div>
                    </div>
                    <div className="live-source-path-field field-stack">
                      <label className="field-label">{text('defaultLabel')}</label>
                      <input
                        type="text"
                        value={path}
                        onChange={(e) => handleChangeLiveSourcePath(source.key, e.target.value)}
                      />
                      <label className="field-label">{text('focusTarget')}</label>
                      <input
                        type="text"
                        value={focusTarget}
                        placeholder={text('focusTargetHint')}
                        onChange={(e) => handleChangeLiveSourceFocusTarget(source.key, e.target.value)}
                      />
                    </div>
                    <div className="live-source-actions">
                      <span className={`status-badge ${enabled ? 'connected' : 'disconnected'}`}>
                        {enabled ? text('enabled') : text('disabled')}
                      </span>
                      <button className="button button-ghost" onClick={() => handleToggleLiveSource(source.key)}>
                        {enabled ? text('disable') : text('enable')}
                      </button>
                      <button className="button button-ghost" onClick={() => handleResetLiveSourcePath(source.key)}>
                        {text('reset')}
                      </button>
                      <button className="button button-primary" onClick={() => handleSaveLiveSourcePath(source.key)}>
                        {text('save')}
                      </button>
                    </div>
                  </div>
                )
              })}
            </div>
          </section>

          <section className="settings-section message-section">
            <div className="section-heading">
              <h2>{text('messageMapping')}</h2>
            </div>
            <div className="message-map">
              {Object.entries(mappings).map(([msgType, state]) => (
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
              <h2>{text('test')}</h2>
            </div>
            <div className="test-actions">
              {Object.keys(STATE_ROWS).map((state) => (
                <button className="button button-ghost" key={state} onClick={() => handleTrigger(state)}>
                  {state}
                </button>
              ))}
              <button className="button button-primary" onClick={handleTriggerNotice}>
                {t('testNotice')}
              </button>
              {NOTICE_TEST_TYPES.map(([noticeType, labelKey]) => (
                <button
                  className="button button-ghost"
                  key={noticeType}
                  onClick={() => handleTriggerNotice(noticeType)}
                >
                  {text(labelKey)}
                </button>
              ))}
            </div>
          </section>

          <section className="settings-section directory-section">
            <div className="section-heading">
              <h2>{text('petDirectory')}</h2>
            </div>
            <div className="directory-row">
              <strong>{text('userPets')}</strong>
              <code>{userPetDir || text('loading')}</code>
            </div>
            <div className="directory-row">
              <strong>{text('builtInPets')}</strong>
              <code>{`{project}/pets/`}</code>
            </div>
            <div className="helper-text">
              {text('petFolderHint').split('\n').map((line) => (
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

export default SettingsWindow
