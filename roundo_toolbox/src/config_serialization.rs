use crate::ErrorWithData;
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;
use toml_edit::{DocumentMut, Item};

pub fn load_or_create_config<T>(dir: &Path, file_name: &str) -> Result<T, ErrorWithData<T>>
where
    T: Serialize + DeserializeOwned + Default,
{
    let path = dir.join(file_name);
    let default_config = T::default();

    if !path.exists() {
        // In extreme condition, there might be TOCTOU issue.
        // Note that TOCTOU is unhandled.
        let Ok(_) = std::fs::write(
            &path,
            toml::to_string_pretty(&default_config).unwrap_or_else(|_| {
                unreachable!("Default_config always exists and should be serializable.")
            }),
        ) else {
            return Err(ErrorWithData {
                data: Some(default_config),
                err_msg: "Failed to write default config file, check IO permission.",
            });
        };

        return Ok(default_config);
    }

    let Ok(text) = std::fs::read_to_string(&path) else {
        return Err(ErrorWithData {
            data: Some(default_config),
            err_msg: "Failed to read config file, check IO permission.",
        });
    };

    // for editing
    let Ok(mut user_doc) = text.parse::<DocumentMut>() else {
        return Err(ErrorWithData {
            data: Some(default_config),
            err_msg: "Failed to parse TOML document, check TOML file.",
        });
    };

    // for read
    let Ok(cfg) = toml::from_str(&text) else {
        return Err(ErrorWithData {
            data: Some(default_config),
            err_msg: "Configuration does not match the expected schema.",
        });
    };

    // default template
    let default_doc = toml::to_string_pretty(&default_config)
        .unwrap_or_else(|_| {
            unreachable!("Default_config always exists and should be serializable.")
        })
        .parse::<DocumentMut>()
        .unwrap_or_else(|_| unreachable!("Default_doc should be a valid TOML document."));

    let changed = merge_missing(default_doc.as_item(), user_doc.as_item_mut());

    if changed {
        let Ok(()) = std::fs::write(&path, user_doc.to_string()) else {
            return Err(ErrorWithData {
                data: Some(default_config),
                err_msg: "Failed to write updated config file, check IO permission.",
            });
        };
    }

    Ok(cfg)
}

fn merge_missing(default: &Item, user: &mut Item) -> bool {
    let mut changed = false;

    let (Some(default_table), Some(user_table)) =
        (default.as_table_like(), user.as_table_like_mut())
    else {
        return false;
    };

    for (key, default_item) in default_table.iter() {
        if !user_table.contains_key(key) {
            user_table.insert(key, default_item.clone());
            changed = true;
            continue;
        }

        let Some(user_item) = user_table.get_mut(key) else {
            unreachable!("User table should contain the key since we checked with contains_key.");
        };

        changed |= merge_missing(default_item, user_item);
    }

    changed
}
