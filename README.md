# fluent-localization
Easy loading of fluent localization resources and generating code bindings for them


# Usage
To make sure bindings are only generated when a localization file changes (to minimize impact on compile times), and so the recommended build.rs can be used without further modifications, it is recommended to make a new crate that only contains your localization bindings.

At runtime localizations are loaded from the "localizations" subdirectory of the current working directory, with "en_US" as default language. This can be overriden with the `TRANSLATION_DIR` and `DEFAULT_LANG` environment variables.

Add a dependency on both crates to load localizations and generate bindings:
```rust
fluent-localization-loader = "1.0"
fluent-localization-bindgen = "1.0"
```

Next make a "localizations" directory inside your project, this should have a subfolder for each language, with in it the fluent localization files. There should also be a "default", this is what will be used to generate the code bindings, and will be used as a fallback if a langauge does not contain a required key. It is recommended to use a symlink for this instead of duplicating a language folder.

To load localizations at runtime you can use

```rust
fluent_localization_loader::LocalizationHolder::load()
```

This will give you a `LocalizationHolder` that holds all localizations for later localizing.


To generate the bindings you use the following code:
```rust
fluent_localization_bindgen::bind_localizations!();
```

At compile time it will insert a struct named `LanguageLocalizer` you can use to localize your strings.
At startup it is recommended to validate that all bound keys are present in the actually loaded resources:
```rust
LanguageLocalizer::validate_default_bundle_complete()?;
```


To localize something you construct a `LanguageLocalizer` with a borrowed `LocalizationHolder`, a language, and then call the ``{filename}_{key}`` helper method on it. It will fall back to the default language if the requested language was not loaded.

Example fluent file (base.ftl)
```ftl
name=English
counter=count is at {$counter}
compounded=Compound exaple, {counter}
```

Would yield would be localized in the following way:
```rust
let localizations = LocalizationHolder::load()?;
let language_localizer = LanguageLocalizer::new(&localizations, "en-US");
println!("{}", language_localizer.base_name());
println!("{}", language_localizer.base_counter(2));
println!("{}", language_localizer.base_compounded(2));
```

And would print ```
English
count is at 2
Compound exaple, count is at 2
```

You can use (a variation) of the following build.rs script to trigger a recompile of your bindings if the resource files change, you might need to move up a directory if your bindings are in a subcrate:

**WARNING**: make sure you got the directory right, this points to a non existant file/folder, rust will always consider this package as needing to be recompiled
```rust
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
    let mut path: PathBuf = env::current_dir().expect("couldn't determine curent dir");
    path.push("localizations");
    path.push("default");

    let handle = fs::read_dir(path).expect("Failed to open localizations base dir");

    for item in handle {
        let item = item.expect("Failed to get handle in localizations dir");
        let underlying_name = item.file_name();
        let name = underlying_name.to_string_lossy();
        if name.ends_with(".ftl") {
            println!("cargo:rerun-if-changed=../../localizations/default/{name}");
        }
    }
}

```