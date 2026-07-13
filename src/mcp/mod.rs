pub mod handlers;

pub use handlers::{
    analyze_log_schema, babysit_pr_schema, evaluate_agent_performance_schema,
    execute_global_refactor_schema, generate_tests_and_scaffolding_schema, handle_analyze_log,
    handle_babysit_pr, handle_evaluate_agent_performance, handle_execute_global_refactor,
    handle_generate_tests_and_scaffolding, handle_query_job_status, handle_scout_context,
    handle_verify_and_triage, handle_web_fetch, registered_mcp_tools, scout_context_schema,
    verify_and_triage_schema, web_fetch_schema, ANALYZE_LOG_TOOL_NAME, BABYSIT_PR_TOOL_NAME,
    EVALUATE_AGENT_PERFORMANCE_TOOL_NAME, EXECUTE_GLOBAL_REFACTOR_TOOL_NAME,
    GENERATE_TESTS_AND_SCAFFOLDING_TOOL_NAME, SCOUT_CONTEXT_TOOL_NAME, VERIFY_AND_TRIAGE_TOOL_NAME,
    WEB_FETCH_TOOL_NAME,
};
