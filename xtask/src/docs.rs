use std::{env, ffi::OsStr, fmt::Write as _, fs, path::Path, process::Command};

use crate::common::{root, run_command, sorted_files};

const SITE_URL: &str = "https://starweaver.wh1isper.top";

pub fn check_docs_examples(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("check-docs-examples takes no arguments".to_string());
    }
    let root = root()?;
    let docs = root.join("docs");
    let mut examples = Vec::new();
    for path in sorted_files(&docs, "md")? {
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let mut rest = text.as_str();
        let mut index = 1_u64;
        while let Some(start) = rest.find("```rust\n") {
            let after = &rest[start + "```rust\n".len()..];
            let end = after
                .find("\n```")
                .ok_or_else(|| format!("unclosed rust fence in {}", path.display()))?;
            let code = &after[..end];
            examples.push(wrap_example(code, &function_name(&path, index)));
            index += 1;
            rest = &after[end + "\n```".len()..];
        }
    }
    if examples.is_empty() {
        return Err("no rust examples found".to_string());
    }
    let tmp = env::temp_dir().join(format!("starweaver-docs-{}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(tmp.join("src")).map_err(|error| error.to_string())?;
    fs::write(
        tmp.join("Cargo.toml"),
        format!(
            r#"[package]
name = "starweaver-docs-examples"
version = "0.0.0"
edition = "2021"

[dependencies]
async-trait = "0.1.89"
schemars = {{ version = "1.2.1", features = ["derive"] }}
serde = {{ version = "1.0.228", features = ["derive"] }}
serde_json = "1.0.145"
starweaver-agent = {{ path = "{}" }}
starweaver-context = {{ path = "{}" }}
starweaver-environment = {{ path = "{}" }}
starweaver-model = {{ path = "{}" }}
starweaver-runtime = {{ path = "{}" }}
starweaver-tools = {{ path = "{}" }}
tokio = {{ version = "1.48.0", features = ["macros", "rt-multi-thread"] }}
"#,
            root.join("crates/starweaver-agent").display(),
            root.join("crates/starweaver-context").display(),
            root.join("crates/starweaver-environment").display(),
            root.join("crates/starweaver-model").display(),
            root.join("crates/starweaver-runtime").display(),
            root.join("crates/starweaver-tools").display(),
        ),
    )
    .map_err(|error| error.to_string())?;
    fs::write(tmp.join("src/lib.rs"), examples.join("\n")).map_err(|error| error.to_string())?;
    let result = run_command(Command::new("cargo").arg("test").current_dir(&tmp));
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn function_name(path: &Path, index: u64) -> String {
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("example");
    let mut sanitized = String::new();
    let mut last_underscore = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch);
            last_underscore = false;
        } else if !last_underscore {
            sanitized.push('_');
            last_underscore = true;
        }
    }
    let trimmed = sanitized.trim_matches('_');
    format!("docs_{trimmed}_{index}")
}

fn wrap_example(code: &str, name: &str) -> String {
    let cleaned = dedent(code).trim().to_string();
    let is_async = cleaned.contains("# async fn example()");
    let visible = cleaned
        .lines()
        .map(|line| line.strip_prefix("# ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    if is_async {
        format!(
            "#[tokio::test]\nasync fn {name}() {{\n{visible}\n    example().await.unwrap();\n}}\n"
        )
    } else {
        format!("#[test]\nfn {name}() {{\n{visible}\n}}\n")
    }
}

fn dedent(text: &str) -> String {
    let lines: Vec<_> = text.lines().collect();
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|ch| ch.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.chars().skip(indent).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn finalize_docs_site() -> Result<(), String> {
    let root = root()?;
    let book = root.join("book");
    if !book.exists() {
        return Err("book directory does not exist; run mdbook build first".to_string());
    }
    let mut urls = Vec::new();
    collect_html(&book, &book, &mut urls)?;
    urls.sort();
    let mut sitemap = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n".to_string();
    for url in &urls {
        writeln!(sitemap, "  <url><loc>{}</loc></url>", escape_xml(url))
            .map_err(|error| error.to_string())?;
    }
    sitemap.push_str("</urlset>\n");
    fs::write(book.join("sitemap.xml"), sitemap).map_err(|error| error.to_string())?;
    fs::write(
        book.join("robots.txt"),
        format!("User-agent: *\nAllow: /\nSitemap: {SITE_URL}/sitemap.xml\n"),
    )
    .map_err(|error| error.to_string())?;
    println!("Wrote sitemap.xml with {} URLs and robots.txt", urls.len());
    Ok(())
}

fn collect_html(root: &Path, dir: &Path, urls: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_html(root, &path, urls)?;
        } else if path.extension() == Some(OsStr::new("html"))
            && path.file_name() != Some(OsStr::new("404.html"))
            && path.file_name() != Some(OsStr::new("toc.html"))
        {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "index.html" {
                urls.push(format!("{SITE_URL}/"));
            } else {
                urls.push(format!("{SITE_URL}/{relative}"));
            }
        }
    }
    Ok(())
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
