#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SCRIPT_PATH="$ROOT_DIR/scripts/zellij-mcp-bootstrap-ssh"

assert_contains() {
  local needle=$1
  local haystack=$2
  local message=$3
  if [[ "$haystack" != *"$needle"* ]]; then
    printf 'assert_contains failed: %s\nmissing: %s\nin: %s\n' "$message" "$needle" "$haystack" >&2
    exit 1
  fi
}

run_missing_alias_test() {
  local stdout_file stderr_file
  stdout_file=$(mktemp)
  stderr_file=$(mktemp)
  if "$SCRIPT_PATH" >"$stdout_file" 2>"$stderr_file"; then
    printf 'expected missing alias invocation to fail\n' >&2
    exit 1
  fi
  assert_contains "missing required <ssh-alias>" "$(<"$stderr_file")" "missing alias should explain usage error"
  rm -f "$stdout_file" "$stderr_file"
}

run_help_test() {
  local output
  output=$("$SCRIPT_PATH" --help)
  assert_contains "Bootstrap a remote host" "$output" "help should describe bootstrap purpose"
  assert_contains "--skip-helper-client" "$output" "help should include helper toggle"
  assert_contains "--helper-cols" "$output" "help should include helper cols option"
  assert_contains "--helper-rows" "$output" "help should include helper rows option"
  assert_contains "readiness_state=READY | AUTO_FIXABLE | MANUAL_ACTION_REQUIRED" "$output" "help should describe repo-owned readiness state output"
  assert_contains "mcp_error_code=PLUGIN_NOT_READY | PROTOCOL_VERSION_MISMATCH | ZJCTL_UNAVAILABLE" "$output" "help should describe repo-owned MCP-facing readiness codes"
}

run_quote_contract_test() {
  local temp_dir fake_ssh local_zjctl_repo stdout_file stderr_file log_file
  temp_dir=$(mktemp -d)
  fake_ssh="$temp_dir/ssh"
  local_zjctl_repo="$temp_dir/local zjctl repo's"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"
  log_file="$temp_dir/ssh.log"
  mkdir -p "$local_zjctl_repo"

  cat >"$fake_ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$BOOTSTRAP_TEST_LOG"
if [[ ${*: -1} == 'printf %s "$HOME"' ]]; then
  printf '%s' "/remote home's"
fi
cat >/dev/null || true
EOF
  chmod +x "$fake_ssh"

  PATH="$temp_dir:$PATH" \
  BOOTSTRAP_TEST_LOG="$log_file" \
  "$SCRIPT_PATH" \
  fake-host \
  --remote-home "/remote home's" \
  --session "sess'one" \
  --helper-session "helper'one" \
  --local-zjctl-repo "$local_zjctl_repo" \
  --skip-plugin-install >"$stdout_file" 2>"$stderr_file"

  assert_contains "remote_bin=/remote home's/.local/bin/zellij_mcp" "$(<"$stdout_file")" "bootstrap should emit remote summary"
  assert_contains "readiness_state=READY" "$(<"$stdout_file")" "bootstrap should emit readiness summary on success"
  assert_contains "readiness_reason=ready" "$(<"$stdout_file")" "bootstrap should mark successful readiness reason"
  assert_contains "mcp_error_code=READY" "$(<"$stdout_file")" "bootstrap should expose success readiness code"
  assert_contains "'/remote home'\\''s/.local/bin'" "$(<"$log_file")" "remote paths should be shell-quoted"
  assert_contains "'sess'\\''one'" "$(<"$log_file")" "session name should be shell-quoted"
  assert_contains "'helper'\\''one'" "$(<"$log_file")" "helper session should be shell-quoted"
  assert_contains "tmux new-session -d -x 160 -y 48 -s 'helper'\\''one'" "$(<"$log_file")" "helper session should start with explicit geometry"
  assert_contains "zjctl panes ls --json >/dev/null" "$(<"$log_file")" "bootstrap should finish with repo-owned readiness probe"

  rm -rf "$temp_dir"
}

run_plugin_build_and_install_test() {
  local temp_dir fake_ssh local_zjctl_repo stdout_file stderr_file log_file
  temp_dir=$(mktemp -d)
  fake_ssh="$temp_dir/ssh"
  local_zjctl_repo="$temp_dir/local-zjctl"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"
  log_file="$temp_dir/ssh.log"
  mkdir -p "$local_zjctl_repo"

  cat >"$fake_ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$BOOTSTRAP_TEST_LOG"
if [[ ${*: -1} == 'printf %s "$HOME"' ]]; then
  printf '%s' "/remote-home"
fi
cat >/dev/null || true
EOF
  chmod +x "$fake_ssh"

  PATH="$temp_dir:$PATH" \
  BOOTSTRAP_TEST_LOG="$log_file" \
  "$SCRIPT_PATH" \
  fake-host \
  --remote-home "/remote-home" \
  --session "phase4" \
  --local-zjctl-repo "$local_zjctl_repo" \
  --skip-helper-client >"$stdout_file" 2>"$stderr_file"

  assert_contains "cargo build-plugin --manifest-path '/remote-home/Documents/git/zellij-skill'/Cargo.toml" "$(<"$log_file")" "bootstrap should build the repo-owned plugin artifact"
  assert_contains "install -m 644 '/remote-home/Documents/git/zellij-skill/target/wasm32-wasip1/release/zrpc.wasm' '/remote-home/.config/zellij/plugins'/zrpc.wasm" "$(<"$log_file")" "bootstrap should install the built plugin artifact directly"
  assert_contains "readiness_state=READY" "$(<"$stdout_file")" "plugin-build path should end in ready summary"
  assert_contains "zjctl panes ls --json >/dev/null" "$(<"$log_file")" "plugin-build path should use repo-owned readiness probe instead of doctor"

  rm -rf "$temp_dir"
}

run_protocol_version_mismatch_readiness_test() {
  local temp_dir fake_ssh local_zjctl_repo stdout_file stderr_file log_file
  temp_dir=$(mktemp -d)
  fake_ssh="$temp_dir/ssh"
  local_zjctl_repo="$temp_dir/local-zjctl"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"
  log_file="$temp_dir/ssh.log"
  mkdir -p "$local_zjctl_repo"

  cat >"$fake_ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$BOOTSTRAP_TEST_LOG"
if [[ ${*: -1} == 'printf %s "$HOME"' ]]; then
  printf '%s' "/remote-home"
  exit 0
fi
if [[ ${*: -1} == *"zjctl panes ls --json >/dev/null"* ]]; then
  printf '%s\n' 'zrpc protocol version mismatch: expected 1, got 0' >&2
  exit 70
fi
cat >/dev/null || true
EOF
  chmod +x "$fake_ssh"

  if PATH="$temp_dir:$PATH" \
    BOOTSTRAP_TEST_LOG="$log_file" \
    "$SCRIPT_PATH" \
    fake-host \
    --remote-home "/remote-home" \
    --session "phase4" \
    --local-zjctl-repo "$local_zjctl_repo" \
    --skip-helper-client >"$stdout_file" 2>"$stderr_file"; then
    printf 'expected protocol-version-mismatch bootstrap proof to fail\n' >&2
    exit 1
  fi

  assert_contains "readiness_state=MANUAL_ACTION_REQUIRED" "$(<"$stdout_file")" "bootstrap should classify protocol mismatch as manual action required"
  assert_contains "readiness_reason=protocol_version_mismatch" "$(<"$stdout_file")" "bootstrap should expose protocol mismatch reason"
  assert_contains "mcp_error_code=PROTOCOL_VERSION_MISMATCH" "$(<"$stdout_file")" "bootstrap should preserve protocol mismatch as distinct MCP-facing code"
  assert_contains "readiness_detail=zrpc protocol version mismatch: expected 1, got 0" "$(<"$stderr_file")" "bootstrap should emit protocol mismatch detail"
  assert_contains "zjctl panes ls --json >/dev/null" "$(<"$log_file")" "protocol mismatch proof should come from repo-owned readiness probe"

  rm -rf "$temp_dir"
}

run_helper_geometry_override_test() {
  local temp_dir fake_ssh local_zjctl_repo stdout_file stderr_file log_file
  temp_dir=$(mktemp -d)
  fake_ssh="$temp_dir/ssh"
  local_zjctl_repo="$temp_dir/local-zjctl"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"
  log_file="$temp_dir/ssh.log"
  mkdir -p "$local_zjctl_repo"

  cat >"$fake_ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$BOOTSTRAP_TEST_LOG"
if [[ ${*: -1} == 'printf %s "$HOME"' ]]; then
  printf '%s' "/remote-home"
fi
cat >/dev/null || true
EOF
  chmod +x "$fake_ssh"

  PATH="$temp_dir:$PATH" \
  BOOTSTRAP_TEST_LOG="$log_file" \
  ZELLIJ_MCP_HELPER_COLS=222 \
  ZELLIJ_MCP_HELPER_ROWS=66 \
  "$SCRIPT_PATH" \
  fake-host \
  --remote-home "/remote-home" \
  --session "phase4" \
  --local-zjctl-repo "$local_zjctl_repo" \
  --skip-plugin-install >"$stdout_file" 2>"$stderr_file"

  assert_contains "tmux new-session -d -x 222 -y 66 -s 'zellij-mcp-client-phase4'" "$(<"$log_file")" "helper geometry should prefer explicit helper env overrides"

  rm -rf "$temp_dir"
}

run_missing_alias_test
run_help_test
run_quote_contract_test
run_plugin_build_and_install_test
run_protocol_version_mismatch_readiness_test
run_helper_geometry_override_test
printf 'ok\n'
