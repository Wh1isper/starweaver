use std::process::Command;

use crate::common::{root, run_capture};

#[derive(Clone, Debug)]
struct FileCoverage {
    path: String,
    lines: u64,
    missed: u64,
    percent: f64,
}

#[derive(Debug)]
struct CoverageGroup {
    packages: &'static [&'static str],
    default_threshold: f64,
    measured_floor: Option<f64>,
    acceptance_paths: &'static [&'static str],
}

fn coverage_group(name: &str) -> Option<CoverageGroup> {
    match name {
        "core" => Some(CoverageGroup {
            packages: &[
                "starweaver-core",
                "starweaver-model",
                "starweaver-runtime",
                "starweaver-tools",
            ],
            default_threshold: 95.0,
            measured_floor: Some(80.0),
            acceptance_paths: &[
                "starweaver-core/src/lib.rs",
                "starweaver-model/src/message.rs",
                "starweaver-model/src/profile.rs",
                "starweaver-model/src/settings.rs",
                "starweaver-model/src/providers/openai_chat.rs",
                "starweaver-runtime/src/agent/runtime_helpers.rs",
                "starweaver-runtime/src/history.rs",
                "starweaver-runtime/src/instructions.rs",
                "starweaver-runtime/src/run.rs",
                "starweaver-tools/src/context.rs",
                "starweaver-tools/src/instruction.rs",
            ],
        }),
        "agent" => Some(CoverageGroup {
            packages: &["starweaver-agent"],
            default_threshold: 90.0,
            measured_floor: None,
            acceptance_paths: &[
                "starweaver-agent/src/",
                "lib.rs",
                "session.rs",
                "subagent.rs",
                "subagent_config.rs",
            ],
        }),
        "service" => Some(CoverageGroup {
            packages: &["starweaver-cli"],
            default_threshold: 80.0,
            measured_floor: None,
            acceptance_paths: &["starweaver-cli/src/", "/starweaver-cli/src/", "main.rs"],
        }),
        _ => None,
    }
}

pub fn coverage_gate(args: &[String]) -> Result<(), String> {
    let group_name = args
        .first()
        .ok_or_else(|| "usage: coverage-gate <core|agent|service> [--threshold N]".to_string())?;
    let group = coverage_group(group_name).ok_or_else(|| "unknown coverage group".to_string())?;
    let mut threshold = group.default_threshold;
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--threshold" && index + 1 < args.len() {
            threshold = args[index + 1]
                .parse::<f64>()
                .map_err(|error| error.to_string())?;
            index += 2;
        } else {
            return Err("usage: coverage-gate <core|agent|service> [--threshold N]".to_string());
        }
    }
    let root = root()?;
    let mut command = Command::new("cargo");
    command.arg("llvm-cov");
    for package in group.packages {
        command.arg("-p").arg(package);
    }
    command
        .arg("--all-features")
        .arg("--locked")
        .arg("--summary-only")
        .current_dir(&root);
    let output = run_capture(&mut command)?;
    print!("{output}");
    let (files, total) = parse_coverage(&output)?;
    if let Some(floor) = group.measured_floor {
        if total.percent < floor {
            return Err(format!(
                "{group_name} measured coverage {:.2}% is below the {:.2}% floor",
                total.percent, floor
            ));
        }
    }
    let acceptance = aggregate_coverage(group.acceptance_paths, &files)?;
    if acceptance.percent < threshold {
        return Err(format!(
            "{group_name} acceptance coverage {:.2}% is below the {:.2}% line gate",
            acceptance.percent, threshold
        ));
    }
    println!(
        "{group_name} acceptance coverage {:.2}% passed the {:.2}% line gate ({}/{} lines)",
        acceptance.percent,
        threshold,
        acceptance.lines - acceptance.missed,
        acceptance.lines
    );
    if group.measured_floor.is_some() {
        println!(
            "{group_name} measured coverage floor: {:.2}%",
            total.percent
        );
    }
    Ok(())
}

fn parse_coverage(output: &str) -> Result<(Vec<FileCoverage>, FileCoverage), String> {
    let mut files = Vec::new();
    let mut total = None;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        let path = parts[0];
        if path != "TOTAL" && !path.ends_with(".rs") {
            continue;
        }
        let Ok(lines) = parts[7].parse::<u64>() else {
            continue;
        };
        let Ok(missed) = parts[8].parse::<u64>() else {
            continue;
        };
        let Ok(percent) = parts[9].trim_end_matches('%').parse::<f64>() else {
            continue;
        };
        let coverage = FileCoverage {
            path: path.to_string(),
            lines,
            missed,
            percent,
        };
        if path == "TOTAL" {
            total = Some(coverage);
        } else {
            files.push(coverage);
        }
    }
    Ok((
        files,
        total.ok_or_else(|| "coverage output did not include a parsable TOTAL line".to_string())?,
    ))
}

fn aggregate_coverage(paths: &[&str], files: &[FileCoverage]) -> Result<FileCoverage, String> {
    let selected: Vec<_> = files
        .iter()
        .filter(|file| {
            paths
                .iter()
                .any(|pattern| path_matches(&file.path, pattern))
        })
        .collect();
    if selected.is_empty() {
        return Err("coverage gate selected no files".to_string());
    }
    let lines = selected.iter().map(|file| file.lines).sum::<u64>();
    let missed = selected.iter().map(|file| file.missed).sum::<u64>();
    let percent = if lines == 0 {
        100.0
    } else {
        ((lines - missed) as f64 / lines as f64) * 100.0
    };
    Ok(FileCoverage {
        path: "acceptance".to_string(),
        lines,
        missed,
        percent,
    })
}

fn path_matches(file_path: &str, pattern: &str) -> bool {
    let file = file_path.trim_matches('/');
    let pattern = pattern.trim_matches('/');
    file == pattern
        || file.starts_with(&format!("{pattern}/"))
        || file.ends_with(&format!("/{pattern}"))
        || format!("/{file}/").contains(&format!("/{pattern}/"))
}
