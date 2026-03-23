#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SCRIPT_PATH="$ROOT_DIR/scripts/zellij-mcp-ssh"

assert_eq() {
  local expected=$1
  local actual=$2
  local message=$3
  if [[ "$expected" != "$actual" ]]; then
    printf 'assert_eq failed: %s\nexpected: %s\nactual: %s\n' "$message" "$expected" "$actual" >&2
    exit 1
  fi
}

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
  assert_eq "" "$(<"$stdout_file")" "missing alias should not write stdout"
  assert_contains "missing required <ssh-alias>" "$(<"$stderr_file")" "missing alias should explain usage error"
  rm -f "$stdout_file" "$stderr_file"
}

run_wrapper_exec_test() {
  local temp_dir fake_bin log_file invoked_args_file stdout_file stderr_file
  temp_dir=$(mktemp -d)
  fake_bin="$temp_dir/remote zellij_mcp"
  log_file="$temp_dir/ssh-args.log"
  invoked_args_file="$temp_dir/remote-invocation.log"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"

  cat >"$fake_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'ZJCTL_BIN=%s\n' "${ZJCTL_BIN-}" >"$WRAPPER_TEST_CAPTURE_DIR/env.log"
printf 'ZELLIJ_MCP_STATE_DIR=%s\n' "${ZELLIJ_MCP_STATE_DIR-}" >>"$WRAPPER_TEST_CAPTURE_DIR/env.log"
printf 'CUSTOM_VAR=%s\n' "${CUSTOM_VAR-}" >>"$WRAPPER_TEST_CAPTURE_DIR/env.log"
for arg in "$@"; do
  printf '%s\n' "$arg"
done >"$WRAPPER_TEST_CAPTURE_DIR/daemon-args.log"
EOF
  chmod +x "$fake_bin"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >"$log_file"
remote_command=${!#}
bash -lc "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  "$SCRIPT_PATH" \
  gpu \
  --remote-bin "$fake_bin" \
  --remote-zjctl-bin "/opt/zjctl remote/bin/zjctl" \
  --remote-state-dir "/var/tmp/zellij mcp/state" \
  --remote-env "CUSTOM_VAR=hello remote's world" \
  --ssh-option "-oConnectTimeout=7" \
  -- --label "hello world's value" >"$stdout_file" 2>"$stderr_file"

  assert_eq "" "$(<"$stdout_file")" "wrapper should not produce stdout noise"
  assert_eq "" "$(<"$stderr_file")" "wrapper should not produce stderr on success"

  local ssh_args
  ssh_args=$(<"$log_file")
  assert_contains "-T" "$ssh_args" "wrapper should disable tty allocation"
  assert_contains "-oBatchMode=yes" "$ssh_args" "wrapper should force batch mode"
  assert_contains "-oConnectTimeout=7" "$ssh_args" "wrapper should forward ssh options"
  assert_contains "gpu" "$ssh_args" "wrapper should pass ssh alias"

  local env_log daemon_args
  env_log=$(<"$temp_dir/env.log")
  daemon_args=$(<"$temp_dir/daemon-args.log")
  assert_contains "ZJCTL_BIN=/opt/zjctl remote/bin/zjctl" "$env_log" "wrapper should export remote ZJCTL_BIN"
  assert_contains "ZELLIJ_MCP_STATE_DIR=/var/tmp/zellij mcp/state" "$env_log" "wrapper should export remote state dir"
  assert_contains "CUSTOM_VAR=hello remote's world" "$env_log" "wrapper should export extra remote env"
  assert_contains "--label" "$daemon_args" "wrapper should forward daemon arg key"
  assert_contains "hello world's value" "$daemon_args" "wrapper should preserve daemon arg value with spaces and quotes"

  rm -rf "$temp_dir"
}

run_missing_alias_test
run_wrapper_exec_test
printf 'ok\n'
