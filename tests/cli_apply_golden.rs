use fastctx::control::agents;
use fastctx::control::codex_config::{self, ExpectedConfig};
use fastctx::control::settings::{Tier, ToolBudgetLevel, ToolBudgets};

#[test]
fn micro_edit_golden_preserves_every_unowned_byte_and_writes_the_exact_private_shape() {
    let original = concat!(
        "# heading\n",
        "custom = 'value'\n",
        "\n",
        "tool_output_token_limit = 9000 # shared\n",
        "\n",
        "[mcp_servers.other]\n",
        "command = 'other'\n",
        "\n",
        "[features.code_mode]\n",
        "direct_only_tool_namespaces = [ 'alpha', 'omega' ]\n",
    );
    let expected = concat!(
        "# heading\n",
        "custom = 'value'\n",
        "\n",
        "tool_output_token_limit = 20000 # shared\n",
        "\n",
        "[mcp_servers.other]\n",
        "command = 'other'\n",
        "\n",
        "[mcp_servers.fastctx]\n",
        "command = \"C:/Users/test/.fastctx/bin/fastctx.exe\"\n",
        "args = [\"serve\"]\n",
        "startup_timeout_sec = 120\n",
        "tool_timeout_sec = 300\n",
        "\n",
        "[mcp_servers.fastctx.env]\n",
        "FASTCTX_TOKEN_BUDGET = \"17000\"\n",
        "FASTCTX_GREP_TOKEN_BUDGET = \"8500\"\n",
        "FASTCTX_GLOB_TOKEN_BUDGET = \"4300\"\n",
        "\n",
        "[features.code_mode]\n",
        "direct_only_tool_namespaces = [ 'alpha', 'omega', \"mcp__fastctx\" ]\n",
    );
    let edit = codex_config::apply(
        original.as_bytes(),
        &ExpectedConfig {
            command: "C:/Users/test/.fastctx/bin/fastctx.exe".to_string(),
            tier: Tier::Standard,
            tool_budgets: ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent50,
                glob: ToolBudgetLevel::Percent25,
                run: ToolBudgetLevel::Inherit,
                job_output: ToolBudgetLevel::Inherit,
            },
            fastshell_enabled: false,
        },
    )
    .unwrap();
    assert_eq!(edit.bytes, expected.as_bytes());
    assert_eq!(edit.conflict.unwrap().current, 9_000);
}

#[test]
fn agents_golden_appends_the_exact_contract_after_one_blank_line() {
    let original = "# User rules\n\nKeep exact.\n";
    let expected = concat!(
        "# User rules\n",
        "\n",
        "Keep exact.\n",
        "\n",
        "<!-- fastctx:begin -->\n",
        "## Local file inspection\n",
        "\n",
        "For reading, searching, and finding local files, prefer the FastCtx MCP\n",
        "tools — `mcp__fastctx__read`, `mcp__fastctx__grep`, `mcp__fastctx__glob` —\n",
        "over `cat`/`Get-Content`, `rg`/`findstr`/`Select-String`, and `dir`/`ls -R`.\n",
        "Read only what the task needs. When you need several files, pass them to\n",
        "one read call as files=[{\"path\": ...}, ...] instead of one call per file.\n",
        "Pass absolute paths. The last line of every result says `Complete` or\n",
        "`Partial` — continue only with the exact parameters a `Partial` note\n",
        "provides.\n",
        "\n",
        "Never point `read_mcp_resource`, `list_mcp_resources`, or\n",
        "`list_mcp_resource_templates` at the `fastctx` server: FastCtx publishes\n",
        "tools, not MCP resources, so those calls always fail. Read a local file\n",
        "with `mcp__fastctx__read` and an absolute path — never a `file://` URI.\n",
        "\n",
        "### Batch replacement\n",
        "\n",
        "Use `mcp__fastctx__replace` for mechanical find-and-replace across files.\n",
        "It preserves each file's encoding and line endings, supports dry-run previews,\n",
        "and rejects concurrent changes before writing. Use apply_patch for generated\n",
        "content, semantic rewrites, or small local edits.\n",
        "<!-- fastctx:end -->\n",
    );
    assert_eq!(
        agents::apply_section(original.as_bytes()).unwrap(),
        expected.as_bytes()
    );
}

#[test]
fn malformed_toml_and_ambiguous_agents_markers_fail_before_producing_bytes() {
    let expected = ExpectedConfig {
        command: "/home/test/.fastctx/bin/fastctx".to_string(),
        tier: Tier::Standard,
        tool_budgets: ToolBudgets::default(),
        fastshell_enabled: false,
    };
    let toml_error = codex_config::apply(b"[broken", &expected).unwrap_err();
    assert!(toml_error.contains("Repair it manually"));
    let agents_error = agents::apply_section(
        b"<!-- fastctx:begin -->\n<!-- fastctx:begin -->\n<!-- fastctx:end -->",
    )
    .unwrap_err();
    assert!(agents_error.contains("duplicate or unmatched"));
}
