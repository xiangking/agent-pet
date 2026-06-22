import { LIVE_SOURCES, PET_SCALE_OPTIONS, STATE_ROWS } from './constants'

function SettingsWindow({
  handleChangeLiveSourceFocusTarget,
  handleChangeLiveSourcePath,
  handleLoadPet,
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
  petScale,
  t,
  userPetDir,
  wsStatus,
}) {
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
              <button className="button button-primary" onClick={handleTriggerNotice}>
                {t('testNotice')}
              </button>
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

export default SettingsWindow
