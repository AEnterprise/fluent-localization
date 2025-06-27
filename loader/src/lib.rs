use std::{collections::HashMap, env, error::Error, fmt::Display, fs, path::PathBuf, sync::Arc};

use fluent_bundle::{FluentResource, bundle::FluentBundle as RawBundle};

use anyhow::{Context, Result};
use fluent_syntax::parser::ParserError;
use intl_memoizer::concurrent::IntlLangMemoizer;
use tracing::{debug, error, trace, warn};
use unic_langid::LanguageIdentifier;

type FluentBundle = RawBundle<Arc<FluentResource>, IntlLangMemoizer>;

pub const FILE_EXTENSION: &str = ".ftl";
pub const DEFAULT_DIR: &str = "default";

///Basic wrapper to hold a resource and its original filename
#[derive(Clone)]
pub struct Resource {
    pub name: String,
    pub resource: Arc<FluentResource>,
}

/// Holder to hold all the loaded bundled for localizations, as well as the currently configured default language
pub struct LocalizationHolder {
    // Store the identifiers as strings so we don't need to convert every time we need to translate something
    pub bundles: HashMap<String, FluentBundle>,
    pub default_language: String,
}
#[derive(Debug)]
pub struct LocalizationLoadingError {
    error: String,
}

impl LocalizationLoadingError {
    pub fn new(error: String) -> Self {
        LocalizationLoadingError { error }
    }
}

impl Error for LocalizationLoadingError {}

impl Display for LocalizationLoadingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

impl LocalizationHolder {
    pub fn load() -> Result<Self> {
        let base_path = base_path();
        debug!(
            "Loading localizations from {}",
            base_path.as_path().to_string_lossy()
        );
        let mut bundles = HashMap::new();

        let default_identifier = get_default_language()?;
        let default = default_identifier.to_string();

        let mut path = base_path.clone();
        path.push(DEFAULT_DIR);

        let defaults = load_resources_from_folder(path)?;

        // Walk through the directory and start assembling our localizer
        let base_handle =
            fs::read_dir(base_path.clone()).context("Failed to read localizations base dir")?;

        for result in base_handle {
            let item_handle = result.context(
                "Failed to get a handle when walking through the localizations directory",
            )?;
            //Store the file name in a var separately before the actual name is stored so it isn't dropped early
            let underlying_name = item_handle.file_name();
            let lang_name = underlying_name.to_string_lossy();

            let meta = item_handle
                .file_type()
                .with_context(|| format!("Failed to get item metadata for {lang_name}"))?;

            if !meta.is_dir() {
                trace!("Skipping {lang_name} because it is not a directory");
                continue;
            }

            let Ok(identifier) = lang_name.parse::<LanguageIdentifier>() else {
                warn!("Skipping {lang_name} because it is not a valid language identifier");
                continue;
            };

            let bundle = load_bundle(base_path.clone(), identifier, defaults.clone())?;

            // finally add the bundle to the map
            bundles.insert(lang_name.to_string(), bundle);
        }

        Ok(LocalizationHolder {
            bundles,
            default_language: default,
        })
    }

    pub fn get_bundle(&self, language: &str) -> &FluentBundle {
        self.bundles
            .get(language)
            .unwrap_or_else(|| self.get_bundle(&self.default_language))
    }

    pub fn get_default_bundle(&self) -> &FluentBundle {
        self.bundles.get(&self.default_language).unwrap()
    }
}

/// The base path localizations will be loaded from, this is controlled by the `TRANSLATION_DIR` environment variable;
/// Will default to the `localizations` subfolder of the current working directory if not set
pub fn base_path() -> PathBuf {
    match env::var("TRANSLATION_DIR") {
        Ok(location) => PathBuf::from(location),
        Err(_) => {
            let mut buf =
                env::current_dir().expect("Failed to get current working directory from std::env");
            buf.push("localizations");
            buf
        }
    }
}

/// Get the current default language, this is controlled by the `DEFAULT_LANG` environment variable.
/// Will default to `DEFAULT` if not set
pub fn get_default_language() -> Result<LanguageIdentifier> {
    let value = env::var("DEFAULT_LANG").unwrap_or("en_US".to_string());
    value
        .parse::<LanguageIdentifier>()
        .with_context(|| format!("Invalid default langauge: {value}"))
}

/// Load all fluent resource files from a directory and returns them.
/// Only files with an .ftl extension will be loaded, does not load files from subfolders
///
/// Generally you don't want to be using this but rather use the load function to get an
/// LocalizationHolder with localizations for all your languages
///
/// However this is public for the purposes of generating bindings through the ... crate, if if you want to do it yourself
/// # Arguments
/// * `path` - A PathBuf to the folder to load the resources from
pub fn load_resources_from_folder(path: PathBuf) -> Result<Vec<Resource>> {
    trace!("Loading resources from {path:?}");
    let p = path.clone();
    let path_name = p.to_string_lossy();
    let mut loaded = Vec::new();

    // Loop over all files in the directory and add them to the bundle
    let lang_dir = fs::read_dir(path)
        .with_context(|| format!("Failed to read localization directory {path_name}"))?;

    for result in lang_dir {
        let item_handle = result.with_context(|| {
            format!("Failed to get a file handle when walking through the {path_name} directory")
        })?;

        let underlying_name = item_handle.file_name();
        let name = underlying_name.to_string_lossy();
        let meta = item_handle
            .file_type()
            .with_context(|| format!("Failed to get item metadata for {path_name}/{name}"))?;

        if !meta.is_file() {
            debug!("Skipping {path_name}/{name} because it is not a file");
            continue;
        }

        if !name.ends_with(FILE_EXTENSION) {
            warn!(
                "Skipping {path_name}/{name} because it doesn't have the proper {FILE_EXTENSION} extension"
            );
            continue;
        }

        trace!("Loading localization file {path_name}/{name}");
        let file_content = fs::read_to_string(item_handle.path())
            .with_context(|| format!("Failed to load localization file {path_name}/{name}"))?;

        let fluent_resource = FluentResource::try_new(file_content.clone())
            .map_err(|(_, error_list)| {
                LocalizationLoadingError::new(fold_displayable(
                    error_list
                        .into_iter()
                        .map(|e| prettify_parse_error(&file_content, e)),
                    "\n-----\n",
                ))
            })
            .with_context(|| format!("Failed to load localization file {path_name}/{name}"))?;

        let arced = Arc::new(fluent_resource);

        loaded.push(Resource {
            name: name.strip_suffix(FILE_EXTENSION).unwrap().to_string(),
            resource: arced,
        })
    }

    Ok(loaded)
}

fn load_bundle(
    mut base_path: PathBuf,
    identifier: LanguageIdentifier,
    defaults: Vec<Resource>,
) -> Result<FluentBundle> {
    let lang_name = identifier.to_string();
    trace!("Loading language {lang_name}");
    base_path.push(&lang_name);

    let mut bundle = FluentBundle::new_concurrent(Vec::from_iter([identifier.clone()]));

    // to test against duplicate keys
    let mut test_bundle = FluentBundle::new_concurrent(Vec::from_iter([identifier]));

    for default in defaults {
        bundle.add_resource_overriding(default.resource)
    }

    for resource in load_resources_from_folder(base_path)? {
        // First we add to the test bundle that does not have defaults, so we get errors if there are duplicate keys across the files (shouldn't happen, but ya know. me proofing)
        test_bundle.add_resource(resource.resource.clone()).map_err(|error_list| {
            LocalizationLoadingError::new(fold_displayable(
        error_list
            .into_iter()
            // This should only ever yield overriding errors so lets keep this simple
            .map(|e| e.to_string()),
        "\n-----\n",
    ))
})
.with_context(|| format!("Failed to load localization file {lang_name}/{} into the duplicate test bundle", resource.name))?;

        bundle.add_resource_overriding(resource.resource)
    }

    Ok(bundle)
}

fn prettify_parse_error(file_content: &str, e: ParserError) -> String {
    // figure out where our line endings are to show something at least a little more useful
    let mut line_endings = file_content.lines().map(|line| (line.len(), line));

    let mut pos = 0;
    let mut line = "";
    let mut line_count = 0;

    loop {
        let Some((len, l)) = line_endings.next() else {
            error!(
                "Somehow fluent-rs reported a error that is past the end of the file? Was at pos {}, looking for {}",
                pos, e.pos.start
            );
            break;
        };

        //store the current line
        line = l;
        line_count += 1;

        // are we there yet?
        if pos + len > e.pos.start {
            break;
        }

        pos += len;
    }

    let line_pos = e.pos.start - pos;

    format!("{} at {line_count}:{line_pos}\n    {line}", e.kind)
}

#[doc(hidden)]
pub fn fold_displayable(
    mut iterator: impl Iterator<Item = impl Display>,
    separator: &str,
) -> String {
    let Some(first) = iterator.next() else {
        return String::new();
    };
    iterator.fold(first.to_string(), |assembled, new| {
        assembled + separator + &new.to_string()
    })
}
