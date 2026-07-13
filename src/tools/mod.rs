mod ast;
mod build_discovery;
mod cmd;
mod compiler;
mod crash_log;
mod env_detector;
mod lang;
mod log_source;
pub mod web_fetch;

pub use ast::{AstUsageFinder, LineRange};
pub use build_discovery::{
    inference_anchor, snapshot_build_context, BuildCommandDiscoverer, LlmBuildDiscoverer,
    NoopBuildDiscoverer, BUILD_DISCOVERY_SYSTEM_PROMPT,
};
pub use cmd::{
    read_file_range, run_fd, run_ripgrep, run_ripgrep_files, run_ripgrep_matching_files,
};
pub use compiler::{edit_file_line, edit_file_range, run_build_command, truncate_build_log};
pub use crash_log::{
    analyze_crash_log, build_summary, parser_confident, read_log_file, strip_file_url, to_report,
    truncate_for_llm, truncate_log_text, CrashAnalysisCore, LogAnalysisReport,
};
pub use log_source::{resolve_log_content, LogSourceKind, ResolvedLog};
pub use env_detector::{find_nearest_module_boundary, get_dirty_files_from_git};
pub use lang::{
    detect_file_language, detect_project_languages, language_from_extension, FileLanguageReport,
    ProjectLanguageReport, SourceLanguage,
};
