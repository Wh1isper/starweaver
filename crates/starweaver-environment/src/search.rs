//! Text search helpers for provider-backed grep operations.

use std::{io, path::Path};

use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, Sink, SinkContext, SinkFinish, SinkMatch};

use crate::{EnvironmentError, EnvironmentResult, FileGrepMatch, FileGrepOptions};

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
    builder.hidden(!include_hidden);
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
    max_results: usize,
    pending_before_context: Vec<(usize, String)>,
    active_match_index: Option<usize>,
}

impl<'a> LocalGrepSink<'a> {
    pub(crate) fn new(
        path: &'a str,
        grep_matches: &'a mut Vec<FileGrepMatch>,
        max_results: usize,
    ) -> Self {
        Self {
            path,
            grep_matches,
            max_results,
            pending_before_context: Vec::new(),
            active_match_index: None,
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

    fn should_accept_match(&self) -> bool {
        self.max_results == 0 || self.grep_matches.len() < self.max_results
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
            .pending_before_context
            .first()
            .map_or(line_number, |(line_number, _)| *line_number);
        let mut context = String::new();
        for (_, line) in self.pending_before_context.drain(..) {
            context.push_str(&line);
        }
        context.push_str(&matching_line);
        self.grep_matches.push(FileGrepMatch {
            path: self.path.to_string(),
            line_number,
            matching_line: matching_line.trim_end_matches('\n').to_string(),
            context,
            context_start_line,
        });
        self.active_match_index = Some(self.grep_matches.len() - 1);
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
                self.pending_before_context
                    .push((Self::line_number(context.line_number()), line));
            }
            grep_searcher::SinkContextKind::After | grep_searcher::SinkContextKind::Other => {
                if let Some(index) = self.active_match_index {
                    if let Some(grep_match) = self.grep_matches.get_mut(index) {
                        grep_match.context.push_str(&line);
                    }
                }
            }
        }
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.pending_before_context.clear();
        self.active_match_index = None;
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
        self.pending_before_context.clear();
        self.active_match_index = None;
        Ok(())
    }
}
