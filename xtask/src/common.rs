use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Map, Value};

pub fn root() -> Result<PathBuf, String> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").map_err(|error| error.to_string())?;
    Path::new(&manifest_dir)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask manifest has no parent".to_string())
}

pub fn run_command(command: &mut Command) -> Result<(), String> {
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed with {status}: {command:?}"))
    }
}

pub fn run_capture(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|error| error.to_string())?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

pub fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    match read_json(path)? {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{} root must be a JSON object", path.display())),
    }
}

pub fn write_json(path: &Path, value: &Value, sort_keys: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let output = if sort_keys {
        serde_json::to_string_pretty(&sort_value(value))
    } else {
        serde_json::to_string_pretty(value)
    }
    .map_err(|error| error.to_string())?;
    fs::write(path, format!("{output}\n")).map_err(|error| error.to_string())
}

pub fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = Map::new();
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted.insert(key.clone(), sort_value(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

pub fn sorted_files(dir: &Path, extension: &str) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension() == Some(OsStr::new(extension)) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
