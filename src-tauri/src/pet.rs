// Codex pet protocol - Pet Loader
// Compatible with: 1536x1872, 8x9 grid, 192x208 cells, WebP/PNG

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const COLUMNS: u32 = 8;
pub const ROWS: u32 = 9;
pub const CELL_WIDTH: u32 = 192;
pub const CELL_HEIGHT: u32 = 208;
pub const ATLAS_WIDTH: u32 = COLUMNS * CELL_WIDTH;
pub const ATLAS_HEIGHT: u32 = ROWS * CELL_HEIGHT;
const APP_CONFIG_DIR: &str = "agent-pet";
const LEGACY_CONFIG_DIR: &str = "codex-pet";
const PET_LIBRARY_RAW_BASE: &str =
    "https://raw.githubusercontent.com/legeling/awesome-codex-pet/main";
const PET_LIBRARY_API_BASE: &str = "https://codex-pets.net/api";
const PET_LIBRARY_PAGE_SIZE: u32 = 30;
const PET_LIBRARY_MAX_PAGE_SIZE: u32 = 60;

/// Animation states matching Codex protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PetState {
    Idle,         // row 0, 6 frames
    RunningRight, // row 1, 8 frames
    RunningLeft,  // row 2, 8 frames
    Waving,       // row 3, 4 frames
    Jumping,      // row 4, 5 frames
    Failed,       // row 5, 8 frames
    Waiting,      // row 6, 6 frames
    Running,      // row 7, 6 frames
    Review,       // row 8, 6 frames
}

impl PetState {
    pub fn row(&self) -> u32 {
        match self {
            PetState::Idle => 0,
            PetState::RunningRight => 1,
            PetState::RunningLeft => 2,
            PetState::Waving => 3,
            PetState::Jumping => 4,
            PetState::Failed => 5,
            PetState::Waiting => 6,
            PetState::Running => 7,
            PetState::Review => 8,
        }
    }

    pub fn frame_count(&self) -> u32 {
        match self {
            PetState::Idle => 6,
            PetState::RunningRight => 8,
            PetState::RunningLeft => 8,
            PetState::Waving => 4,
            PetState::Jumping => 5,
            PetState::Failed => 8,
            PetState::Waiting => 6,
            PetState::Running => 6,
            PetState::Review => 6,
        }
    }

    pub fn durations(&self) -> Vec<u64> {
        match self {
            PetState::Idle => vec![280, 110, 110, 140, 140, 320],
            PetState::RunningRight => vec![120, 120, 120, 120, 120, 120, 120, 220],
            PetState::RunningLeft => vec![120, 120, 120, 120, 120, 120, 120, 220],
            PetState::Waving => vec![140, 140, 140, 280],
            PetState::Jumping => vec![140, 140, 140, 140, 280],
            PetState::Failed => vec![140, 140, 140, 140, 140, 140, 140, 240],
            PetState::Waiting => vec![150, 150, 150, 150, 150, 260],
            PetState::Running => vec![120, 120, 120, 120, 120, 220],
            PetState::Review => vec![150, 150, 150, 150, 150, 280],
        }
    }
}

impl std::fmt::Display for PetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                PetState::Idle => "idle",
                PetState::RunningRight => "running-right",
                PetState::RunningLeft => "running-left",
                PetState::Waving => "waving",
                PetState::Jumping => "jumping",
                PetState::Failed => "failed",
                PetState::Waiting => "waiting",
                PetState::Running => "running",
                PetState::Review => "review",
            }
        )
    }
}

impl std::str::FromStr for PetState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(PetState::Idle),
            "running-right" => Ok(PetState::RunningRight),
            "running-left" => Ok(PetState::RunningLeft),
            "waving" => Ok(PetState::Waving),
            "jumping" => Ok(PetState::Jumping),
            "failed" => Ok(PetState::Failed),
            "waiting" => Ok(PetState::Waiting),
            "running" => Ok(PetState::Running),
            "review" => Ok(PetState::Review),
            _ => Err(format!("Unknown state: {}", s)),
        }
    }
}

/// Pet configuration matching Codex pet.json format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetConfig {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub spritesheet_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spritesheet_data_url: Option<String>,
    #[serde(default)]
    pub message_map: HashMap<String, String>,
    #[serde(default)]
    pub state_durations: HashMap<String, u64>,
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            id: "claude".to_string(),
            display_name: "Claude".to_string(),
            description: "A tiny orange blocky digital pet with stubby side arms, short legs, and cute pixel-art expressions.".to_string(),
            spritesheet_path: "spritesheet.webp".to_string(),
            spritesheet_data_url: None,
            message_map: crate::message::default_message_map(),
            state_durations: HashMap::new(),
        }
    }
}

/// Pet info for listing
#[derive(Debug, Clone, Serialize)]
pub struct PetInfo {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub has_spritesheet: bool,
    pub thumbnail_data_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RemotePetCatalogItem {
    slug: String,
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    author_handle: String,
    #[serde(default)]
    author_url: String,
    #[serde(default)]
    primary_category: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OnlinePetCatalogItem {
    id: String,
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    spritesheet_path: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    owner_handle: String,
    #[serde(default)]
    owner_name: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    spritesheet_url: String,
    #[serde(default)]
    poster_url: String,
    #[serde(default)]
    preview_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OnlinePetCatalogPage {
    page: u32,
    page_size: u32,
    pets: Vec<OnlinePetCatalogItem>,
    total: u32,
    total_pages: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct OnlinePetDetail {
    pet: OnlinePetCatalogItem,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetLibraryItem {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub thumbnail_url: String,
    pub author: String,
    pub author_handle: String,
    pub author_url: String,
    pub category: String,
    pub license: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetLibraryPage {
    pub items: Vec<PetLibraryItem>,
    pub page: u32,
    pub page_size: u32,
    pub total: u32,
    pub total_pages: u32,
    pub from_cache: bool,
}

#[derive(Error, Debug)]
pub enum PetError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Pet not found: {0}")]
    NotFound(String),
    #[error("Invalid spritesheet: {0}")]
    InvalidSpritesheet(String),
    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
}

/// Get the user pets directory: ~/.config/agent-pet/pets/
pub fn user_pets_dir() -> Result<PathBuf, PetError> {
    let config =
        dirs::config_dir().ok_or_else(|| PetError::NotFound("Config dir not found".to_string()))?;
    let dir = config.join(APP_CONFIG_DIR).join("pets");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn legacy_user_pets_dir() -> Result<Option<PathBuf>, PetError> {
    let config =
        dirs::config_dir().ok_or_else(|| PetError::NotFound("Config dir not found".to_string()))?;
    let dir = config.join(LEGACY_CONFIG_DIR).join("pets");
    Ok(dir.exists().then_some(dir))
}

fn user_pet_dirs() -> Result<Vec<PathBuf>, PetError> {
    let current = user_pets_dir()?;
    let mut dirs = vec![current.clone()];

    if let Some(legacy) = legacy_user_pets_dir()? {
        let current_canonical = current.canonicalize().ok();
        let legacy_canonical = legacy.canonicalize().ok();
        if legacy_canonical.is_some() && legacy_canonical != current_canonical {
            dirs.push(legacy);
        }
    }

    Ok(dirs)
}

/// Get the built-in pets directory (project-root/pets/)
pub fn builtin_pets_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points to src-tauri/; pets/ is at the project root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pets")
}

fn validate_pet_id(pet_id: &str) -> Result<(), PetError> {
    let path = Path::new(pet_id);
    let mut components = path.components();

    match (components.next(), components.next()) {
        (Some(Component::Normal(id)), None) if !id.is_empty() => Ok(()),
        _ => Err(PetError::NotFound(format!("Invalid pet id: {}", pet_id))),
    }
}

fn validate_library_pet_id(pet_id: &str) -> Result<(), PetError> {
    validate_pet_id(pet_id)?;

    if pet_id.starts_with('-')
        || pet_id.ends_with('-')
        || !pet_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(PetError::NotFound(format!(
            "Invalid library pet id: {}",
            pet_id
        )));
    }

    Ok(())
}

fn remote_pet_asset_url(pet_id: &str, filename: &str) -> String {
    format!("{PET_LIBRARY_RAW_BASE}/pets/{pet_id}/{filename}")
}

fn pet_library_cache_dir() -> Result<PathBuf, PetError> {
    let cache =
        dirs::cache_dir().ok_or_else(|| PetError::NotFound("Cache dir not found".to_string()))?;
    let dir = cache.join(APP_CONFIG_DIR).join("pet-library");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn pet_library_page_cache_path(page: u32, page_size: u32, sort: &str) -> Result<PathBuf, PetError> {
    let safe_sort: String = sort
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect();
    Ok(pet_library_cache_dir()?.join(format!("{safe_sort}-page-{page}-size-{page_size}.json")))
}

fn temp_install_dir(user_dir: &Path, pet_id: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    user_dir.join(format!(".installing-{pet_id}-{stamp}"))
}

fn installed_pet_ids() -> std::collections::HashSet<String> {
    list_pets_sync()
        .unwrap_or_default()
        .into_iter()
        .filter(|pet| pet.has_spritesheet)
        .map(|pet| pet.id)
        .collect()
}

async fn fetch_pet_catalog() -> Result<Vec<RemotePetCatalogItem>, PetError> {
    let url = format!("{PET_LIBRARY_RAW_BASE}/pets.json");
    Ok(reqwest::get(url)
        .await?
        .error_for_status()?
        .json::<Vec<RemotePetCatalogItem>>()
        .await?)
}

fn normalize_library_page(page: u32) -> u32 {
    page.max(1)
}

fn normalize_library_page_size(page_size: u32) -> u32 {
    page_size.clamp(1, PET_LIBRARY_MAX_PAGE_SIZE)
}

async fn fetch_online_pet_catalog_page(
    page: u32,
    page_size: u32,
    sort: &str,
) -> Result<OnlinePetCatalogPage, PetError> {
    let url = format!("{PET_LIBRARY_API_BASE}/pets?page={page}&pageSize={page_size}&sort={sort}");
    Ok(reqwest::get(url)
        .await?
        .error_for_status()?
        .json::<OnlinePetCatalogPage>()
        .await?)
}

async fn fetch_online_pet_detail(pet_id: &str) -> Result<OnlinePetCatalogItem, PetError> {
    let url = format!("{PET_LIBRARY_API_BASE}/pets/{pet_id}");
    Ok(reqwest::get(url)
        .await?
        .error_for_status()?
        .json::<OnlinePetDetail>()
        .await?
        .pet)
}

fn read_cached_online_pet_catalog_page(
    page: u32,
    page_size: u32,
    sort: &str,
) -> Result<Option<OnlinePetCatalogPage>, PetError> {
    let cache_path = pet_library_page_cache_path(page, page_size, sort)?;
    if !cache_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(cache_path)?;
    Ok(Some(serde_json::from_str::<OnlinePetCatalogPage>(
        &content,
    )?))
}

fn write_cached_online_pet_catalog_page(
    page: &OnlinePetCatalogPage,
    sort: &str,
) -> Result<(), PetError> {
    let cache_path = pet_library_page_cache_path(page.page, page.page_size, sort)?;
    std::fs::write(cache_path, serde_json::to_vec(page)?)?;
    Ok(())
}

async fn get_online_pet_catalog_page(
    page: u32,
    page_size: u32,
    sort: &str,
) -> Result<(OnlinePetCatalogPage, bool), PetError> {
    let page = normalize_library_page(page);
    let page_size = normalize_library_page_size(page_size);

    match fetch_online_pet_catalog_page(page, page_size, sort).await {
        Ok(catalog_page) => {
            let _ = write_cached_online_pet_catalog_page(&catalog_page, sort);
            Ok((catalog_page, false))
        }
        Err(error) => match read_cached_online_pet_catalog_page(page, page_size, sort)? {
            Some(catalog_page) => Ok((catalog_page, true)),
            None => Err(error),
        },
    }
}

fn validate_spritesheet_extension(path: &Path) -> Result<(), PetError> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match ext.as_deref() {
        Some("png") | Some("webp") => Ok(()),
        _ => Err(PetError::InvalidSpritesheet(
            "Spritesheet must be a .png or .webp file".to_string(),
        )),
    }
}

fn spritesheet_mime(path: &Path) -> Result<&'static str, PetError> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());

    match ext.as_deref() {
        Some("png") => Ok("image/png"),
        Some("webp") => Ok("image/webp"),
        _ => Err(PetError::InvalidSpritesheet(
            "Spritesheet must be a .png or .webp file".to_string(),
        )),
    }
}

fn spritesheet_data_url(path: &Path) -> Result<String, PetError> {
    let mime = spritesheet_mime(path)?;
    let bytes = std::fs::read(path)?;
    Ok(format!("data:{};base64,{}", mime, BASE64.encode(bytes)))
}

fn spritesheet_thumbnail_data_url(path: &Path) -> Result<String, PetError> {
    validate_spritesheet(path)?;
    let image = image::open(path)?;
    let thumbnail = image.crop_imm(0, 0, CELL_WIDTH, CELL_HEIGHT);
    let mut bytes = Cursor::new(Vec::new());
    thumbnail.write_to(&mut bytes, ImageFormat::Png)?;
    Ok(format!(
        "data:image/png;base64,{}",
        BASE64.encode(bytes.into_inner())
    ))
}

fn canonical_child_path(base: &Path, candidate: &Path) -> Result<PathBuf, PetError> {
    let base = base.canonicalize()?;
    let candidate = candidate.canonicalize()?;

    if !candidate.starts_with(&base) {
        return Err(PetError::InvalidSpritesheet(
            "Spritesheet path must stay inside the pet directory".to_string(),
        ));
    }

    Ok(candidate)
}

fn resolve_spritesheet_path(pet_dir: &Path, configured_path: &str) -> Result<PathBuf, PetError> {
    let mut candidates = vec![pet_dir.join(configured_path)];
    candidates.push(pet_dir.join("spritesheet.webp"));
    candidates.push(pet_dir.join("spritesheet.png"));

    for candidate in candidates {
        if candidate.exists() {
            let path = canonical_child_path(pet_dir, &candidate)?;
            validate_spritesheet(&path)?;
            return Ok(path);
        }
    }

    Err(PetError::InvalidSpritesheet(
        "No valid spritesheet found".to_string(),
    ))
}

/// Scan a single directory for pets and add to the vec
fn scan_pet_dir(dir: &Path, pets: &mut Vec<PetInfo>, seen: &mut std::collections::HashSet<String>) {
    if !dir.exists() {
        return;
    }

    for entry in std::fs::read_dir(dir).into_iter().flatten() {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.is_dir() {
                let config_path = path.join("pet.json");

                if let Ok(content) = std::fs::read_to_string(&config_path) {
                    if let Ok(config) = serde_json::from_str::<PetConfig>(&content) {
                        let folder_id = path.file_name().and_then(|name| name.to_str());
                        if folder_id != Some(config.id.as_str())
                            || validate_pet_id(&config.id).is_err()
                        {
                            continue;
                        }

                        let spritesheet = resolve_spritesheet_path(&path, &config.spritesheet_path);
                        let has_spritesheet = spritesheet.is_ok();
                        let thumbnail_data_url = spritesheet
                            .ok()
                            .and_then(|path| spritesheet_thumbnail_data_url(&path).ok());

                        // Deduplicate by id; user dir takes precedence over builtin
                        if seen.insert(config.id.clone()) {
                            pets.push(PetInfo {
                                id: config.id,
                                display_name: config.display_name,
                                description: config.description,
                                has_spritesheet,
                                thumbnail_data_url,
                            });
                        }
                    }
                }
            }
        }
    }
}

fn list_pets_sync() -> Result<Vec<PetInfo>, PetError> {
    let mut pets = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for user_dir in user_pet_dirs().unwrap_or_default() {
        scan_pet_dir(&user_dir, &mut pets, &mut seen);
    }

    let builtin = builtin_pets_dir();
    scan_pet_dir(&builtin, &mut pets, &mut seen);

    Ok(pets)
}

/// List all available pets (user dir + built-in dir)
pub async fn list_pets(_app: tauri::AppHandle) -> Result<Vec<PetInfo>, PetError> {
    list_pets_sync()
}

pub async fn list_pet_library(page: Option<u32>) -> Result<PetLibraryPage, PetError> {
    let requested_page = normalize_library_page(page.unwrap_or(1));
    match list_online_pet_library_page(requested_page, PET_LIBRARY_PAGE_SIZE).await {
        Ok(library_page) => Ok(library_page),
        Err(_) if requested_page == 1 => list_legacy_pet_library_page().await,
        Err(error) => Err(error),
    }
}

async fn list_online_pet_library_page(
    page: u32,
    page_size: u32,
) -> Result<PetLibraryPage, PetError> {
    let (catalog_page, from_cache) =
        get_online_pet_catalog_page(page, page_size, "popular").await?;
    let installed = installed_pet_ids();

    let items = catalog_page
        .pets
        .into_iter()
        .filter(|item| validate_library_pet_id(&item.id).is_ok())
        .map(|item| {
            let category = if item.kind.trim().is_empty() {
                item.tags.first().cloned().unwrap_or_default()
            } else {
                item.kind
            };
            let description = if item.description.trim().is_empty() {
                "A downloadable desktop pet.".to_string()
            } else {
                item.description
            };
            let author = if item.owner_name.trim().is_empty() {
                item.owner_handle.clone()
            } else {
                item.owner_name
            };

            PetLibraryItem {
                installed: installed.contains(&item.id),
                thumbnail_url: if item.poster_url.trim().is_empty() {
                    item.preview_url
                } else {
                    item.poster_url
                },
                id: item.id,
                display_name: item.display_name,
                description,
                author,
                author_handle: item.owner_handle,
                author_url: String::new(),
                category,
                license: String::new(),
            }
        })
        .collect();

    Ok(PetLibraryPage {
        items,
        page: catalog_page.page,
        page_size: catalog_page.page_size,
        total: catalog_page.total,
        total_pages: catalog_page.total_pages,
        from_cache,
    })
}

async fn list_legacy_pet_library_page() -> Result<PetLibraryPage, PetError> {
    let catalog = fetch_pet_catalog().await?;
    let installed = installed_pet_ids();
    let total = catalog.len() as u32;

    let items = catalog
        .into_iter()
        .filter(|item| validate_library_pet_id(&item.slug).is_ok())
        .map(|item| {
            let description = item
                .description
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| {
                    let category = item.primary_category.trim();
                    if category.is_empty() {
                        "A downloadable desktop pet.".to_string()
                    } else {
                        format!("A downloadable desktop pet in the {category} collection.")
                    }
                });

            PetLibraryItem {
                installed: installed.contains(&item.slug),
                thumbnail_url: remote_pet_asset_url(&item.slug, "spritesheet.webp"),
                id: item.slug,
                display_name: item.name,
                description,
                author: item.author,
                author_handle: item.author_handle,
                author_url: item.author_url,
                category: item.primary_category,
                license: item.license,
            }
        })
        .collect();

    Ok(PetLibraryPage {
        items,
        page: 1,
        page_size: total,
        total,
        total_pages: 1,
        from_cache: false,
    })
}

async fn find_online_pet(pet_id: &str) -> Result<OnlinePetCatalogItem, PetError> {
    if let Ok(item) = fetch_online_pet_detail(pet_id).await {
        if validate_library_pet_id(&item.id).is_ok() {
            return Ok(item);
        }
    }

    let mut page = 1;

    loop {
        let (catalog_page, _) =
            get_online_pet_catalog_page(page, PET_LIBRARY_MAX_PAGE_SIZE, "popular").await?;
        if let Some(item) = catalog_page.pets.into_iter().find(|item| item.id == pet_id) {
            return Ok(item);
        }

        if page >= catalog_page.total_pages {
            break;
        }
        page += 1;
    }

    Err(PetError::NotFound(format!(
        "Pet is not available in the library: {}",
        pet_id
    )))
}

async fn install_online_pet_from_library(pet_id: &str) -> Result<PetInfo, PetError> {
    validate_library_pet_id(pet_id)?;

    let item = find_online_pet(pet_id).await?;
    if item.spritesheet_url.trim().is_empty() {
        return Err(PetError::InvalidSpritesheet(
            "Library pet is missing a spritesheet URL".to_string(),
        ));
    }

    if let Ok(config) = load_pet_config(pet_id).await {
        let thumbnail_data_url = PathBuf::from(&config.spritesheet_path)
            .parent()
            .map(|dir| dir.join("spritesheet.webp"))
            .and_then(|path| spritesheet_thumbnail_data_url(&path).ok());

        return Ok(PetInfo {
            id: config.id,
            display_name: config.display_name,
            description: config.description,
            has_spritesheet: true,
            thumbnail_data_url,
        });
    }

    let user_dir = user_pets_dir()?;
    let target_dir = user_dir.join(pet_id);
    let temp_dir = temp_install_dir(&user_dir, pet_id);

    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    std::fs::create_dir_all(&temp_dir)?;

    let result = async {
        let spritesheet_bytes = reqwest::get(item.spritesheet_url.clone())
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let config = PetConfig {
            id: item.id.clone(),
            display_name: item.display_name.clone(),
            description: if item.description.trim().is_empty() {
                "A downloadable desktop pet.".to_string()
            } else {
                item.description.clone()
            },
            spritesheet_path: "spritesheet.webp".to_string(),
            spritesheet_data_url: None,
            message_map: crate::message::default_message_map(),
            state_durations: HashMap::new(),
        };
        let config_bytes = serde_json::to_vec_pretty(&config)?;
        let config_path = temp_dir.join("pet.json");
        let spritesheet_path = temp_dir.join("spritesheet.webp");
        std::fs::write(&config_path, &config_bytes)?;
        std::fs::write(&spritesheet_path, &spritesheet_bytes)?;

        let config: PetConfig = serde_json::from_slice(&config_bytes)?;
        if config.id != pet_id {
            return Err(PetError::NotFound(format!(
                "Library pet id '{}' does not match config '{}'",
                pet_id, config.id,
            )));
        }
        validate_pet_id(&config.id)?;
        resolve_spritesheet_path(&temp_dir, &config.spritesheet_path)?;

        Ok::<PetConfig, PetError>(config)
    }
    .await;

    match result {
        Ok(config) => {
            if target_dir.exists() {
                std::fs::remove_dir_all(&target_dir)?;
            }
            std::fs::rename(&temp_dir, &target_dir)?;

            Ok(PetInfo {
                id: config.id,
                display_name: config.display_name,
                description: config.description,
                has_spritesheet: true,
                thumbnail_data_url: spritesheet_thumbnail_data_url(
                    &target_dir.join("spritesheet.webp"),
                )
                .ok(),
            })
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            Err(error)
        }
    }
}

async fn install_legacy_pet_from_library(pet_id: &str) -> Result<PetInfo, PetError> {
    validate_library_pet_id(pet_id)?;

    let catalog = fetch_pet_catalog().await?;
    if !catalog
        .iter()
        .any(|item| item.slug == pet_id && validate_library_pet_id(&item.slug).is_ok())
    {
        return Err(PetError::NotFound(format!(
            "Pet is not available in the library: {}",
            pet_id
        )));
    }

    let user_dir = user_pets_dir()?;
    let target_dir = user_dir.join(pet_id);
    let temp_dir = temp_install_dir(&user_dir, pet_id);

    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    std::fs::create_dir_all(&temp_dir)?;

    let result = async {
        let config_bytes = reqwest::get(remote_pet_asset_url(pet_id, "pet.json"))
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let spritesheet_bytes = reqwest::get(remote_pet_asset_url(pet_id, "spritesheet.webp"))
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let config_path = temp_dir.join("pet.json");
        let spritesheet_path = temp_dir.join("spritesheet.webp");
        std::fs::write(&config_path, &config_bytes)?;
        std::fs::write(&spritesheet_path, &spritesheet_bytes)?;

        let config: PetConfig = serde_json::from_slice(&config_bytes)?;
        if config.id != pet_id {
            return Err(PetError::NotFound(format!(
                "Library pet id '{}' does not match config '{}'",
                pet_id, config.id,
            )));
        }
        validate_pet_id(&config.id)?;
        resolve_spritesheet_path(&temp_dir, &config.spritesheet_path)?;

        Ok::<PetConfig, PetError>(config)
    }
    .await;

    match result {
        Ok(config) => {
            if target_dir.exists() {
                std::fs::remove_dir_all(&target_dir)?;
            }
            std::fs::rename(&temp_dir, &target_dir)?;

            Ok(PetInfo {
                id: config.id,
                display_name: config.display_name,
                description: config.description,
                has_spritesheet: true,
                thumbnail_data_url: spritesheet_thumbnail_data_url(
                    &target_dir.join("spritesheet.webp"),
                )
                .ok(),
            })
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            Err(error)
        }
    }
}

pub async fn install_pet_from_library(pet_id: &str) -> Result<PetInfo, PetError> {
    match install_online_pet_from_library(pet_id).await {
        Ok(info) => Ok(info),
        Err(_) => install_legacy_pet_from_library(pet_id).await,
    }
}

/// Try to resolve a pet directory from user dir or built-in dir
fn resolve_pet_dir(pet_id: &str) -> Result<PathBuf, PetError> {
    validate_pet_id(pet_id)?;

    // 1. Check user dirs first
    for user_dir in user_pet_dirs().unwrap_or_default() {
        let user_dir = user_dir.canonicalize()?;
        let pet_dir = user_dir.join(pet_id);
        if pet_dir.exists() {
            let pet_dir = pet_dir.canonicalize()?;
            if pet_dir.is_dir() && pet_dir.starts_with(&user_dir) {
                return Ok(pet_dir);
            }
        }
    }

    // 2. Fall back to built-in dir
    let builtin = builtin_pets_dir().canonicalize()?;
    let pet_dir = builtin.join(pet_id);
    if pet_dir.exists() {
        let pet_dir = pet_dir.canonicalize()?;
        if pet_dir.is_dir() && pet_dir.starts_with(&builtin) {
            return Ok(pet_dir);
        }
    }

    Err(PetError::NotFound(pet_id.to_string()))
}

/// Load a specific pet configuration
pub async fn load_pet_config(pet_id: &str) -> Result<PetConfig, PetError> {
    let pet_dir = resolve_pet_dir(pet_id)?;

    let config_path = pet_dir.join("pet.json");
    let content = std::fs::read_to_string(&config_path)?;
    let mut config: PetConfig = serde_json::from_str(&content)?;
    if config.id != pet_id {
        return Err(PetError::NotFound(format!(
            "Pet id '{}' does not match folder '{}'",
            config.id, pet_id,
        )));
    }

    let spritesheet = resolve_spritesheet_path(&pet_dir, &config.spritesheet_path)?;
    config.spritesheet_data_url = Some(spritesheet_data_url(&spritesheet)?);
    config.spritesheet_path = spritesheet.to_string_lossy().to_string();

    Ok(config)
}

/// Validate spritesheet dimensions
pub fn validate_spritesheet(path: &Path) -> Result<(u32, u32), PetError> {
    if !path.exists() {
        return Err(PetError::InvalidSpritesheet("File not found".to_string()));
    }

    validate_spritesheet_extension(path)?;

    let dimensions = image::image_dimensions(path)?;
    if dimensions != (ATLAS_WIDTH, ATLAS_HEIGHT) {
        return Err(PetError::InvalidSpritesheet(format!(
            "Expected {}x{}, got {}x{}",
            ATLAS_WIDTH, ATLAS_HEIGHT, dimensions.0, dimensions.1,
        )));
    }

    Ok(dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PetState 常量校验 ─────────────────────────────────────

    #[test]
    fn atlas_dimensions_consistency() {
        assert_eq!(ATLAS_WIDTH, COLUMNS * CELL_WIDTH);
        assert_eq!(ATLAS_HEIGHT, ROWS * CELL_HEIGHT);
    }

    #[test]
    fn all_states_have_unique_rows() {
        let states = all_pet_states();
        let rows: Vec<u32> = states.iter().map(|s| s.row()).collect();
        let row_set: std::collections::HashSet<u32> = rows.iter().copied().collect();
        assert_eq!(rows.len(), row_set.len(), "存在重复的 row 编号");
    }

    // ── PetState::row() ───────────────────────────────────────

    #[test]
    fn pet_state_rows_sequential() {
        let states = all_pet_states();
        for s in &states {
            let row = s.row();
            assert!(
                row < ROWS,
                "状态 {} 的 row {} 超出范围 (ROWS={})",
                s,
                row,
                ROWS
            );
        }
    }

    // ── PetState::frame_count() ───────────────────────────────

    #[test]
    fn frame_count_within_grid() {
        let states = all_pet_states();
        for s in &states {
            let frames = s.frame_count();
            assert!(frames > 0, "状态 {} 帧数为 0", s);
            assert!(
                frames <= COLUMNS,
                "状态 {} 帧数 {} 超出列数 {}",
                s,
                frames,
                COLUMNS
            );
        }
    }

    // ── PetState::durations() ─────────────────────────────────

    #[test]
    fn durations_length_matches_frame_count() {
        let states = all_pet_states();
        for s in &states {
            let durations = s.durations();
            let frames = s.frame_count() as usize;
            assert_eq!(
                durations.len(),
                frames,
                "状态 {} 的 durations 长度 ({}) 与 frame_count ({}) 不匹配",
                s,
                durations.len(),
                frames,
            );
        }
    }

    #[test]
    fn all_durations_are_positive() {
        let states = all_pet_states();
        for s in &states {
            for (i, d) in s.durations().iter().enumerate() {
                assert!(*d > 0, "状态 {} 的第 {} 帧持续时间为 0", s, i);
            }
        }
    }

    // ── Display / FromStr 往返 ────────────────────────────────

    #[test]
    fn display_fromstr_roundtrip() {
        let states = all_pet_states();
        for state in &states {
            let display_str = state.to_string();
            let parsed: PetState = display_str.parse().unwrap();
            assert_eq!(*state, parsed, "状态 {} 的 Display/FromStr 往返失败", state);
        }
    }

    #[test]
    fn fromstr_rejects_unknown() {
        let result = "nonexistent".parse::<PetState>();
        assert!(result.is_err(), "未知状态名应返回 Err");
    }

    // ── PetConfig 默认值 ─────────────────────────────────────

    #[test]
    fn pet_config_default_has_correct_id() {
        let config = PetConfig::default();
        assert_eq!(config.id, "claude");
        assert_eq!(config.display_name, "Claude");
    }

    #[test]
    fn pet_config_default_message_map_matches_module_function() {
        let config = PetConfig::default();
        let module_map = crate::message::default_message_map();
        // PetConfig 默认的 message_map 应与 message 模块的 default_message_map 一致
        for (key, val) in &module_map {
            assert_eq!(
                config.message_map.get(key),
                Some(val),
                "PetConfig 默认 message_map 中 '{}' 的值与 default_message_map 不一致",
                key,
            );
        }
    }

    // ── PetConfig JSON 序列化 / 反序列化 ──────────────────────

    #[test]
    fn pet_config_json_roundtrip() {
        let config = PetConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PetConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, config.id);
        assert_eq!(parsed.display_name, config.display_name);
        assert_eq!(parsed.message_map.len(), config.message_map.len());
    }

    #[test]
    fn pet_config_camel_case_serde() {
        let json = r#"{
            "id": "test-pet",
            "displayName": "Test Pet",
            "description": "A test",
            "spritesheetPath": "sprites.webp"
        }"#;
        let config: PetConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.id, "test-pet");
        assert_eq!(config.display_name, "Test Pet");
        assert_eq!(config.spritesheet_path, "sprites.webp");
    }

    // ── builtin_pets_dir ──────────────────────────────────────

    #[test]
    fn builtin_pets_dir_points_to_pets_subdir() {
        let dir = builtin_pets_dir();
        assert!(
            dir.ends_with("pets"),
            "builtin_pets_dir 应以 'pets' 结尾: {:?}",
            dir
        );
    }

    // ── scan_pet_dir 边界情况 ─────────────────────────────────

    #[test]
    fn scan_pet_dir_nonexistent_returns_empty() {
        let dir = PathBuf::from("/nonexistent/path/that/does/not/exist");
        let mut pets = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_pet_dir(&dir, &mut pets, &mut seen);
        assert!(pets.is_empty());
    }

    // ── validate_spritesheet ──────────────────────────────────

    #[test]
    fn validate_spritesheet_missing_file() {
        let result = validate_spritesheet(Path::new("/nonexistent/sheet.webp"));
        assert!(result.is_err());
    }

    #[test]
    fn validate_spritesheet_rejects_unsupported_extension() {
        let dir = std::env::temp_dir().join(format!("agent-pet-test-{}", std::process::id(),));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spritesheet.txt");
        std::fs::write(&path, b"not an image").unwrap();

        let result = validate_spritesheet(&path);
        assert!(matches!(result, Err(PetError::InvalidSpritesheet(_))));

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn validate_spritesheet_accepts_builtin_claude_dimensions() {
        let path = builtin_pets_dir().join("claude").join("spritesheet.webp");
        let dimensions = validate_spritesheet(&path).unwrap();
        assert_eq!(dimensions, (ATLAS_WIDTH, ATLAS_HEIGHT));
    }

    // ── 路径安全 ──────────────────────────────────────────────

    #[test]
    fn validate_pet_id_rejects_path_traversal() {
        assert!(validate_pet_id("../claude").is_err());
        assert!(validate_pet_id("nested/claude").is_err());
        assert!(validate_pet_id("").is_err());
    }

    #[test]
    fn resolve_pet_dir_rejects_path_traversal() {
        assert!(resolve_pet_dir("../claude").is_err());
    }

    #[tokio::test]
    async fn load_builtin_claude_resolves_canonical_spritesheet() {
        let config = load_pet_config("claude").await.unwrap();
        let spritesheet = PathBuf::from(config.spritesheet_path);
        assert!(spritesheet.is_absolute());
        assert!(spritesheet.starts_with(builtin_pets_dir().canonicalize().unwrap()));
    }

    // ── PetError ──────────────────────────────────────────────

    #[test]
    fn pet_error_display() {
        let err = PetError::NotFound("my-pet".to_string());
        assert_eq!(format!("{}", err), "Pet not found: my-pet");

        let err = PetError::InvalidSpritesheet("bad dims".to_string());
        assert_eq!(format!("{}", err), "Invalid spritesheet: bad dims");
    }

    #[test]
    fn pet_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let pet_err: PetError = io_err.into();
        assert!(format!("{}", pet_err).contains("IO error"));
    }

    #[test]
    fn pet_error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let pet_err: PetError = json_err.into();
        assert!(format!("{}", pet_err).contains("JSON parse error"));
    }

    // ── helpers ───────────────────────────────────────────────

    fn all_pet_states() -> Vec<PetState> {
        vec![
            PetState::Idle,
            PetState::RunningRight,
            PetState::RunningLeft,
            PetState::Waving,
            PetState::Jumping,
            PetState::Failed,
            PetState::Waiting,
            PetState::Running,
            PetState::Review,
        ]
    }
}
