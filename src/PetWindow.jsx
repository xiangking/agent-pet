import { ATLAS_HEIGHT, ATLAS_WIDTH } from './constants'

function PetWindow({
  bubble,
  currentState,
  currentFrame,
  displayHeight,
  displayWidth,
  getBackgroundPosition,
  handleBubbleClick,
  handlePetMouseDown,
  handleTrigger,
  petScale,
  spritesheet,
  t,
  windowHeight,
  windowWidth,
}) {
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

export default PetWindow
