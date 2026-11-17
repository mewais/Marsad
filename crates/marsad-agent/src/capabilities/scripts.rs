//! Pure discovery shell-script builders + the section splitter/parsers (no IO).
//!
//! Extracted from `capabilities.rs` verbatim; `use super::*` inherits the
//! parent module's imports and cross-section helpers.

use super::*;

// ===========================================================================
// Part 1b — discovery SHELL SCRIPT builders + section parser (all PURE, no IO)
//
// Each driver builds ONE discovery script that runs the host's `setup` first
// (so `command -v`, `codex debug models`, the claude env, etc. see the SAME
// environment a launched session would), then emits sections delimited by
// unique sentinel markers. `run_local`/`run_remote_script_bounded` executes the
// script; `parse_discovery_sections` splits the output back into named sections
// with NO IO. Every builder + the splitter is unit-tested (the crate's 100% bar
// stays on this pure layer; the real exec paths are `cfg(not(coverage))`).
// ===========================================================================

/// Sentinel that opens a named discovery section. The name is a fixed ASCII
/// keyword (`avail`/`models`/`sessions`/`env`) chosen by the builders — never
/// user input — so it can never collide with shell metacharacters.
const SECTION_BEGIN: &str = "@@MARSAD_SEC_BEGIN:";
/// Sentinel that closes a named discovery section.
const SECTION_END: &str = "@@MARSAD_SEC_END:";

/// POSIX single-quote a string for safe embedding in a `bash -l -s` script.
/// (Kept in-crate rather than reused from `remote.rs`, whose `remote_shell_quote`
/// is `pub(crate)` but lives behind `cfg(not(coverage))`-adjacent code; this one
/// is always compiled + unit-tested so the pure builders keep the crate's 100%
/// bar.)
pub(crate) fn sh_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Emit the shell that prints a section: a BEGIN marker line, the body of
/// `cmd` (run in a subshell so a failure doesn't abort the outer script), then
/// an END marker line. `name` is a fixed keyword; `cmd` is verbatim shell.
pub(crate) fn section(name: &str, cmd: &str) -> String {
    format!(
        "echo '{SECTION_BEGIN}{name}'\n{cmd}\necho '{SECTION_END}{name}'\n",
        SECTION_BEGIN = SECTION_BEGIN,
        SECTION_END = SECTION_END,
        name = name,
        cmd = cmd,
    )
}

/// Prepend the host's `setup` (if any) to a discovery script body. The setup runs
/// in the SAME shell as the sections so its env/PATH mutations are visible to
/// `command -v` and the models subcommands. A failing setup is tolerated
/// (`|| true`) so discovery still reports availability honestly rather than
/// aborting with an empty result.
pub(crate) fn with_setup(setup: Option<&str>, body: &str) -> String {
    match setup.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("{{ {s}; }} 2>/dev/null || true\n{body}", s = s, body = body),
        None => body.to_string(),
    }
}

/// The first whitespace-separated token of a resolved command line — the WORD
/// that `command -v` checks for availability (e.g. `codex --sandbox` → `codex`,
/// `/opt/genie/genie` → `/opt/genie/genie`). Empty/whitespace-only input yields
/// an empty string (the caller treats an empty word as "unavailable").
pub fn command_word(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or("")
}

/// The `wasl` discovery section: `command -v wasl-mcpd`. Host-global (NOT
/// per-driver) — appended to EVERY driver's discovery script so each (driver,
/// host) caps snapshot carries whether wasl-mcpd is installed on that host. The
/// launch UI gates a managed `wasl-*` driver on BOTH its native CLI AND this.
/// Parsed by `parse_avail` (same shape as the native `avail` section).
pub(crate) fn wasl_avail_section() -> String {
    section("wasl", "command -v wasl-mcpd 2>/dev/null")
}

/// Build the claude discovery script: after `setup`, check availability of the
/// resolved command's first word, then print the three `ANTHROPIC_DEFAULT_*`
/// env values (the host's real versioned models — claude has no "list models"
/// command, so the env is the only truthful per-host source). Three additional
/// sections surface custom/subscription models: `ANTHROPIC_MODEL` env override,
/// `"model"` fields in the claude config JSON files, and `"model":"..."` values
/// observed in recent transcript JSONL files. Sessions are listed per requested
/// `project` (claude stores transcripts under `~/.claude/projects/<encoded-cwd>/`),
/// newest-first, one `mtime_ms<TAB>uuid<TAB><first user line json>` record each —
/// the same shape `parse_remote_claude_session_list` already parses.
pub fn build_claude_discovery_script(
    command: &str,
    setup: Option<&str>,
    project: Option<&str>,
) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    // Print each family's resolved env value on its own labelled line so the
    // parser can map family → versioned id.
    let env = section(
        "env",
        "printf 'opus\\t%s\\n' \"${ANTHROPIC_DEFAULT_OPUS_MODEL:-}\"\n\
         printf 'sonnet\\t%s\\n' \"${ANTHROPIC_DEFAULT_SONNET_MODEL:-}\"\n\
         printf 'haiku\\t%s\\n' \"${ANTHROPIC_DEFAULT_HAIKU_MODEL:-}\"",
    );
    // Surface any custom/subscription model from three host sources.
    //
    // Source 1: ANTHROPIC_MODEL env — the primary single-model override used by
    // subscription and custom setups (e.g. "fable" pinned this way).
    // ANTHROPIC_SMALL_FAST_MODEL is a secondary fast-path override.
    let extra_models = section(
        "extra_models",
        "base=${CLAUDE_CONFIG_DIR:-$HOME/.claude}\n\
         printf 'model\\t%s\\n' \"${ANTHROPIC_MODEL:-}\"\n\
         printf 'model\\t%s\\n' \"${ANTHROPIC_SMALL_FAST_MODEL:-}\"\n\
         \n\
         # Source 2: \"model\" fields in claude config JSON files (no jq dependency).\n\
         grep -ho '\"model\"[[:space:]]*:[[:space:]]*\"[^\"]*\"' \
           \"$base/settings.json\" \"$HOME/.claude.json\" 2>/dev/null \
           | sed 's/.*\"\\([^\"]*\\)\"$/\\1/' \
           | while IFS= read -r m; do printf 'model\\t%s\\n' \"$m\"; done\n\
         \n\
         # Source 3: models observed in recent transcripts — most likely to surface\n\
         # a subscription model like \"fable\" that is only stored in session history.\n\
         # Bounded so it can't dominate the probe deadline on a slow remote FS:\n\
         # only the newest few files, each read for just its FIRST model match\n\
         # (grep -m1, so a huge transcript isn't scanned end-to-end).\n\
         ls -t \"$base\"/projects/*/*.jsonl 2>/dev/null | head -6 \
           | while IFS= read -r f; do \
               grep -m1 -h -o '\"model\":\"[^\"]*\"' \"$f\" 2>/dev/null; \
             done \
           | sed 's/\"model\":\"\\([^\"]*\\)\"/\\1/' \
           | sort -u | head -20 \
           | while IFS= read -r m; do printf 'model\\t%s\\n' \"$m\"; done",
    );
    // Version: capture `claude --version` output (first line) so the probe can
    // derive the available permission modes via `claude_available_modes(version)`.
    // Best-effort: if claude isn't installed or --version fails, the section body
    // is empty and the caller leaves `available_modes` empty (loses the mode picker
    // for this host, but never errors).
    let version = section(
        "version",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} --version 2>/dev/null | head -1 || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    // Sessions: only when a concrete project is in scope (claude's on-disk dir is
    // the project's real cwd, `/`→non-alnum encoded). Resolve the cwd on the host
    // and encode it there, matching `encode_project_dir` exactly.
    let sessions = match project {
        Some(p) if !p.trim().is_empty() => {
            let cd = cd_for(p);
            let body = format!(
                "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\n\
                 cwd=$(cd {cd} 2>/dev/null && pwd -P)\n\
                 enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g')\n\
                 dir=\"$base/projects/$enc\"\n\
                 printf 'MARSAD_DIAG\\tccd=[%s]|base=[%s]|cwd=[%s]|enc=[%s]|exists=%s|count=%s\\n' \
                   \"${{CLAUDE_CONFIG_DIR:-UNSET}}\" \"$base\" \"$cwd\" \"$enc\" \
                   \"$([ -d \"$dir\" ] && echo Y || echo N)\" \
                   \"$(ls \"$dir\"/*.jsonl 2>/dev/null | wc -l | tr -d ' ')\"\n\
                 for f in $(ls -t \"$dir\"/*.jsonl 2>/dev/null); do \
                   u=$(basename \"$f\" .jsonl); \
                   m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
                   l=$(grep -m1 '\"type\":\"user\"' \"$f\" 2>/dev/null | head -c 4000); \
                   printf '%s000\\t%s\\t%s\\n' \"$m\" \"$u\" \"$l\"; \
                 done",
                cd = cd
            );
            section("sessions", &body)
        }
        _ => section("sessions", "true"),
    };
    let wasl = wasl_avail_section();
    with_setup(
        setup,
        &format!("{avail}{wasl}{version}{env}{extra_models}{sessions}"),
    )
}

/// Build the codex discovery script: after `setup`, availability of the resolved
/// command word, then `<command> debug models` (JSON parsed by
/// `parse_codex_debug_models`), then the newest codex rollout sessions under
/// `~/.codex/sessions` printed as `mtime_ms<TAB>filename<TAB><first NDJSON line>`
/// records (parsed by `parse_remote_codex_session_list`).
pub fn build_codex_discovery_script(command: &str, setup: Option<&str>) -> String {
    let word = command_word(command);
    let avail = section("avail", &format!("command -v {} 2>/dev/null", sh_squote(word)));
    // Run the models subcommand against the RESOLVED command (verbatim, so flags
    // like `--sandbox` are preserved) only if the word resolves.
    let models = section(
        "models",
        &format!(
            "command -v {w} >/dev/null 2>&1 && {cmd} debug models 2>/dev/null || true",
            w = sh_squote(word),
            cmd = command,
        ),
    );
    let sessions = section(
        "sessions",
        "root=\"$HOME/.codex/sessions\"\n\
         for f in $(ls -t \"$root\"/*/*/*/rollout-*.jsonl 2>/dev/null | head -200); do \
           b=$(basename \"$f\"); \
           m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
           l=$(head -1 \"$f\" 2>/dev/null | head -c 4000); \
           printf '%s000\\t%s\\t%s\\n' \"$m\" \"$b\" \"$l\"; \
         done",
    );
    let wasl = wasl_avail_section();
    with_setup(setup, &format!("{avail}{wasl}{models}{sessions}"))
}

/// A parsed discovery output: the named sections extracted between the
/// BEGIN/END sentinels. Unknown/duplicate section names keep the LAST body
/// (probes only emit each name once). Text outside any section (setup noise,
/// login-shell banners) is discarded.
#[derive(Debug, Default, PartialEq)]
pub struct DiscoverySections {
    pub sections: HashMap<String, String>,
}

impl DiscoverySections {
    /// Body of `name`, or `""` when absent.
    pub fn get(&self, name: &str) -> &str {
        self.sections.get(name).map(String::as_str).unwrap_or("")
    }
}

/// Split raw discovery stdout into named sections by the BEGIN/END sentinels.
/// A BEGIN with no matching END (truncated output / a killed probe) yields
/// whatever body accumulated up to EOF — honest partial data, never a panic.
/// Lines outside any open section are ignored.
pub fn parse_discovery_sections(raw: &str) -> DiscoverySections {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix(SECTION_BEGIN) {
            // A new BEGIN closes any dangling open section first.
            if let Some((n, body)) = current.take() {
                sections.insert(n, body.join("\n"));
            }
            current = Some((name.trim().to_string(), Vec::new()));
        } else if let Some(name) = line.strip_prefix(SECTION_END) {
            if let Some((n, body)) = current.take() {
                // Only close if the END names the open section; a mismatched END
                // still closes (defensive) — the name recorded is the OPEN one.
                let _ = name;
                sections.insert(n, body.join("\n"));
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line.to_string());
        }
    }
    // A truncated output: flush the still-open section.
    if let Some((n, body)) = current.take() {
        sections.insert(n, body.join("\n"));
    }
    DiscoverySections { sections }
}

/// Interpret an `avail` section body: availability is "the resolved command word
/// printed a non-empty path" (`command -v` prints the resolved path on success,
/// nothing on failure). Trims; any non-empty line means available.
pub fn parse_avail(body: &str) -> bool {
    body.lines().any(|l| !l.trim().is_empty())
}

/// Parse the claude `env` section (`family<TAB>value` lines) into the
/// family→resolved-id map `build_claude_model_catalog` consumes. Empty values
/// (env unset) are dropped so a family with no host env carries no annotation.
pub fn parse_claude_env_models(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in body.lines() {
        let mut parts = line.splitn(2, '\t');
        let family = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if !family.is_empty() && !value.is_empty() {
            out.insert(family.to_string(), value.to_string());
        }
    }
    out
}

/// Parse the claude `version` section body into a plain version string suitable for
/// `marsad_events::claude_available_modes(version)`. The section body is the first
/// line of `claude --version`, which typically looks like `"1.0.42 (claude-code)"`.
/// Returns the first non-empty trimmed line, or an empty string on missing/empty body.
pub fn parse_claude_version(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Parse the claude `extra_models` section (`model<TAB>value` lines) into a
/// deduplicated list of extra model ids. Empty values, family words
/// (opus/sonnet/haiku), and synthetic placeholders (empty or containing `<`)
/// are dropped. The order is preserved; dedup removes later duplicates.
pub fn parse_claude_extra_models(body: &str) -> Vec<String> {
    const FAMILIES: &[&str] = &["opus", "sonnet", "haiku"];
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for line in body.lines() {
        let mut parts = line.splitn(2, '\t');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if key != "model" {
            continue;
        }
        // Drop empties, synthetic placeholders, and known family words.
        if value.is_empty() || value.contains('<') || FAMILIES.contains(&value) {
            continue;
        }
        if seen.insert(value.to_string()) {
            out.push(value.to_string());
        }
    }
    out
}

/// Parse the codex `sessions` section: one
/// `mtime_ms<TAB>filename<TAB><first NDJSON line>` record per line (newest-first,
/// as emitted). Reuses `parse_codex_rollout_meta` (filename → id+project) and
/// `extract_codex_user_snippet` for the snippet. A malformed mtime or a non-
/// rollout filename skips the record. Order is preserved.
pub fn parse_remote_codex_session_list(raw: &str) -> Vec<DiscoveredSession> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut parts = line.splitn(3, '\t');
        let mtime_str = parts.next().unwrap_or("");
        let filename = parts.next().unwrap_or("");
        let first_line = parts.next().unwrap_or("");
        let modified_ms: u64 = match mtime_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some((id, project)) = parse_codex_rollout_meta(filename, first_line) {
            out.push(DiscoveredSession {
                id,
                project,
                snippet: extract_codex_user_snippet(first_line),
                modified_ms,
            });
        }
    }
    out
}

/// `cd` target for a discovery script's project path, expanding a leading `~`
/// (mirrors `remote.rs::cd_target`; kept in-crate + tested so the pure builders
/// stay in the coverage layer).
fn cd_for(project: &str) -> String {
    if project == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = project.strip_prefix("~/") {
        format!("\"$HOME/{rest}\"")
    } else {
        sh_squote(project)
    }
}

