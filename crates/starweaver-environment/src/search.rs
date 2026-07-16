//! Text search helpers for provider-backed grep operations.

use std::{collections::VecDeque, io, path::Path};

use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, Sink, SinkContext, SinkFinish, SinkMatch};

use crate::{
    EnvironmentError, EnvironmentResult, FileGrepMatch, FileGrepOptions, include_path,
    normalize_path,
};

pub fn search_text(
    path: &str,
    content: &str,
    regex_matcher: &RegexMatcher,
    context_lines: usize,
    max_matches_per_file: usize,
    max_results: usize,
    grep_matches: &mut Vec<FileGrepMatch>,
) -> EnvironmentResult<()> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut file_matches = 0;
    for (index, line) in lines.iter().enumerate() {
        if max_results > 0 && grep_matches.len() >= max_results {
            break;
        }
        if max_matches_per_file > 0 && file_matches >= max_matches_per_file {
            break;
        }
        if regex_matcher
            .is_match(line.as_bytes())
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
        {
            let start_index = index.saturating_sub(context_lines);
            let end_index = (index + context_lines + 1).min(lines.len());
            grep_matches.push(FileGrepMatch {
                path: path.to_string(),
                line_number: index + 1,
                matching_line: line.trim_end_matches('\n').to_string(),
                context: lines[start_index..end_index].concat(),
                context_start_line: start_index + 1,
            });
            file_matches += 1;
        }
    }
    Ok(())
}

pub fn local_search_walk_builder(
    search_root: &Path,
    include_hidden: bool,
    include_ignored: bool,
) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(search_root);
    builder.hidden(false);
    if !include_hidden {
        let search_root = search_root.to_path_buf();
        builder.filter_entry(move |entry| {
            let Ok(relative) = entry.path().strip_prefix(&search_root) else {
                return false;
            };
            if relative.as_os_str().is_empty() {
                return true;
            }
            let relative = normalize_path(relative);
            if !include_path(&relative, false) {
                return false;
            }
            relative != ".agents"
                || entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir())
        });
    }
    builder.ignore(!include_ignored);
    builder.git_ignore(!include_ignored);
    builder.git_global(!include_ignored);
    builder.git_exclude(!include_ignored);
    builder.require_git(false);
    builder.follow_links(false);
    builder
}

pub fn local_grep_file_match_limit(
    options: &FileGrepOptions,
    current_matches: usize,
) -> Option<u64> {
    let remaining_total = options
        .max_results
        .checked_sub(current_matches)
        .filter(|_| options.max_results > 0);
    match (options.max_matches_per_file, remaining_total) {
        (0, None) => None,
        (0, Some(remaining)) => Some(remaining as u64),
        (per_file, None) => Some(per_file as u64),
        (per_file, Some(remaining)) => Some(per_file.min(remaining) as u64),
    }
}

pub struct LocalGrepSink<'a> {
    path: &'a str,
    grep_matches: &'a mut Vec<FileGrepMatch>,
    context_lines: usize,
    max_results: usize,
    recent_lines: VecDeque<(usize, String)>,
    active_matches: Vec<(usize, usize)>,
}

impl<'a> LocalGrepSink<'a> {
    pub(crate) const fn new(
        path: &'a str,
        grep_matches: &'a mut Vec<FileGrepMatch>,
        context_lines: usize,
        max_results: usize,
    ) -> Self {
        Self {
            path,
            grep_matches,
            context_lines,
            max_results,
            recent_lines: VecDeque::new(),
            active_matches: Vec::new(),
        }
    }

    fn line_string(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn line_number(line_number: Option<u64>) -> usize {
        line_number
            .and_then(|line_number| usize::try_from(line_number).ok())
            .unwrap_or(1)
    }

    const fn should_accept_match(&self) -> bool {
        self.max_results == 0 || self.grep_matches.len() < self.max_results
    }

    fn push_recent_line(&mut self, line_number: usize, line: String) {
        if self.context_lines == 0 {
            return;
        }
        self.recent_lines.push_back((line_number, line));
        while self.recent_lines.len() > self.context_lines {
            self.recent_lines.pop_front();
        }
    }

    fn push_after_context_line(&mut self, line_number: usize, line: &str) {
        if self.context_lines == 0 {
            return;
        }
        self.active_matches.retain(|(_, match_line_number)| {
            line_number.saturating_sub(*match_line_number) <= self.context_lines
        });
        for (index, match_line_number) in &self.active_matches {
            if line_number > *match_line_number
                && line_number - *match_line_number <= self.context_lines
                && let Some(grep_match) = self.grep_matches.get_mut(*index)
            {
                grep_match.context.push_str(line);
            }
        }
        self.active_matches.retain(|(_, match_line_number)| {
            line_number.saturating_sub(*match_line_number) < self.context_lines
        });
    }
}

impl Sink for LocalGrepSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if !self.should_accept_match() {
            return Ok(false);
        }
        let line_number = Self::line_number(mat.line_number());
        let matching_line = Self::line_string(mat.bytes());
        let context_start_line = self
            .recent_lines
            .front()
            .map_or(line_number, |(line_number, _)| *line_number);
        let mut context = String::new();
        for (_, line) in &self.recent_lines {
            context.push_str(line);
        }
        context.push_str(&matching_line);
        self.grep_matches.push(FileGrepMatch {
            path: self.path.to_string(),
            line_number,
            matching_line: matching_line.trim_end_matches('\n').to_string(),
            context,
            context_start_line,
        });
        let match_index = self.grep_matches.len() - 1;
        self.push_after_context_line(line_number, &matching_line);
        self.push_recent_line(line_number, matching_line);
        if self.context_lines > 0 {
            self.active_matches.push((match_index, line_number));
        }
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let line = Self::line_string(context.bytes());
        match context.kind() {
            grep_searcher::SinkContextKind::Before => {
                self.push_recent_line(Self::line_number(context.line_number()), line);
            }
            grep_searcher::SinkContextKind::After | grep_searcher::SinkContextKind::Other => {
                let line_number = Self::line_number(context.line_number());
                self.push_after_context_line(line_number, &line);
                self.push_recent_line(line_number, line);
            }
        }
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.recent_lines.clear();
        self.active_matches.clear();
        Ok(true)
    }

    fn binary_data(
        &mut self,
        _searcher: &Searcher,
        _binary_byte_offset: u64,
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn finish(&mut self, _searcher: &Searcher, _finish: &SinkFinish) -> Result<(), Self::Error> {
        self.recent_lines.clear();
        self.active_matches.clear();
        Ok(())
    }
}
