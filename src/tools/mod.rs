mod ast;
mod build_discovery;
mod cmd;
mod compiler;
mod env_detector;
mod lang;
pub mod web_fetch;

pub use ast::AstUsageFinder;
pub use build_discovery::{
    inference_anchor, snapshot_build_context, BuildCommandDiscoverer, LlmBuildDiscoverer,
    NoopBuildDiscoverer, BUILD_DISCOVERY_SYSTEM_PROMPT,
};
pub use cmd::{read_file_range, run_fd, run_ripgrep};
pub use compiler::{edit_file_line, run_build_command, truncate_build_log};
pub use env_detector::{find_nearest_module_boundary, get_dirty_files_from_git};
pub use lang::{
    detect_file_language, detect_project_languages, language_from_extension, FileLanguageReport,
    ProjectLanguageReport, SourceLanguage,
};
