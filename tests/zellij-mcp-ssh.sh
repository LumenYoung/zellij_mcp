#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SCRIPT_PATH="$ROOT_DIR/scripts/zellij-mcp-ssh"

daemon_stdio_command() {
  printf '%s' "cargo run --quiet --bin zellij_mcp --"
}

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

assert_not_contains() {
  local needle=$1
  local haystack=$2
  local message=$3
  if [[ "$haystack" == *"$needle"* ]]; then
    printf 'assert_not_contains failed: %s\nunexpected: %s\nin: %s\n' "$message" "$needle" "$haystack" >&2
    exit 1
  fi
}

json_string_field() {
  local key=$1
  local file=$2
  local line
  line=$(grep -m1 "\"$key\"" "$file" || true)
  if [[ -z "$line" ]]; then
    printf 'json_string_field failed: missing key %s in %s\n' "$key" "$file" >&2
    exit 1
  fi
  line=${line#*\"$key\": \"}
  line=${line%%\"*}
  printf '%s' "$line"
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

run_alias_only_path_discovery_test() {
  local temp_dir remote_home remote_bin_dir log_file stdout_file stderr_file
  temp_dir=$(mktemp -d)
  remote_home="$temp_dir/remote-home"
  remote_bin_dir="$remote_home/.local/bin"
  log_file="$temp_dir/ssh-args.log"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"

  mkdir -p "$remote_bin_dir"

  cat >"$remote_bin_dir/zjctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
  chmod +x "$remote_bin_dir/zjctl"

  cat >"$remote_bin_dir/zellij_mcp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'PATH=%s\n' "$PATH" >"$WRAPPER_TEST_CAPTURE_DIR/env.log"
printf 'ZJCTL_BIN=%s\n' "${ZJCTL_BIN-}" >>"$WRAPPER_TEST_CAPTURE_DIR/env.log"
printf 'SELF_RESOLVED=%s\n' "$(command -v zellij_mcp)" >>"$WRAPPER_TEST_CAPTURE_DIR/env.log"
printf 'ZJCTL_RESOLVED=%s\n' "$(command -v "$ZJCTL_BIN")" >>"$WRAPPER_TEST_CAPTURE_DIR/env.log"
for arg in "$@"; do
  printf '%s\n' "$arg"
done >"$WRAPPER_TEST_CAPTURE_DIR/daemon-args.log"
EOF
  chmod +x "$remote_bin_dir/zellij_mcp"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >"$log_file"
remote_command=${!#}
printf '%s\n' "$remote_command" >"${WRAPPER_TEST_CAPTURE_DIR:?}/remote-command.log"
PATH=${WRAPPER_TEST_REMOTE_PATH:?} HOME=${WRAPPER_TEST_REMOTE_HOME:?} bash -lc "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="$remote_bin_dir:/usr/local/bin:/usr/bin:/bin" \
  "$SCRIPT_PATH" \
  gpu \
  --remote-zjctl-bin "zjctl" \
  --ssh-option "-oConnectTimeout=7" \
  -- --probe ready >"$stdout_file" 2>"$stderr_file"

  assert_eq "" "$(<"$stdout_file")" "alias-only wrapper proof should not produce stdout noise"
  assert_eq "" "$(<"$stderr_file")" "alias-only wrapper proof should not produce stderr on success"

  local ssh_args remote_command env_log daemon_args
  ssh_args=$(<"$log_file")
  remote_command=$(<"$temp_dir/remote-command.log")
  env_log=$(<"$temp_dir/env.log")
  daemon_args=$(<"$temp_dir/daemon-args.log")

  assert_contains "-T" "$ssh_args" "alias-only wrapper should disable tty allocation"
  assert_contains "-oBatchMode=yes" "$ssh_args" "alias-only wrapper should force batch mode"
  assert_contains "-oConnectTimeout=7" "$ssh_args" "alias-only wrapper should forward ssh options"
  assert_contains "gpu" "$ssh_args" "alias-only wrapper should pass ssh alias"
  assert_contains "'zellij_mcp'" "$remote_command" "alias-only wrapper should invoke the remote binary by name"
  assert_contains "'ZJCTL_BIN=zjctl'" "$remote_command" "alias-only wrapper should preserve alias-only zjctl export"
  assert_contains "PATH=$remote_bin_dir" "$env_log" "alias-only remote proof should run with user-space bin directory on PATH"
  assert_contains "ZJCTL_BIN=zjctl" "$env_log" "alias-only remote proof should export alias-only zjctl"
  assert_contains "SELF_RESOLVED=$remote_bin_dir/zellij_mcp" "$env_log" "alias-only remote proof should resolve zellij_mcp from user-space PATH"
  assert_contains "ZJCTL_RESOLVED=$remote_bin_dir/zjctl" "$env_log" "alias-only remote proof should resolve zjctl from user-space PATH"
  assert_contains "--probe" "$daemon_args" "alias-only wrapper should forward daemon arg key"
  assert_contains "ready" "$daemon_args" "alias-only wrapper should forward daemon arg value"

  rm -rf "$temp_dir"
}

run_manual_action_required_readiness_test() {
  local temp_dir remote_bin_dir stdout_file stderr_file
  temp_dir=$(mktemp -d)
  remote_bin_dir="$temp_dir/remote-bin"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"

  mkdir -p "$remote_bin_dir"

  cat >"$remote_bin_dir/zellij_mcp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'waiting on a Zellij plugin permission prompt\n' >&2
printf 'approve the plugin permissions before retrying\n' >&2
exit 70
EOF
  chmod +x "$remote_bin_dir/zellij_mcp"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
remote_command=${!#}
bash -lc "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  if PATH="$temp_dir:$remote_bin_dir:$PATH" "$SCRIPT_PATH" gpu >"$stdout_file" 2>"$stderr_file"; then
    printf 'expected manual-action-required readiness proof to fail\n' >&2
    exit 1
  fi

  assert_eq "" "$(<"$stdout_file")" "manual-action-required readiness proof should not emit stdout"

  local stderr_output
  stderr_output=$(<"$stderr_file")
  assert_contains "waiting on a Zellij plugin permission prompt" "$stderr_output" "manual-action-required readiness proof should surface the plugin prompt block"
  assert_contains "approve the plugin permissions before retrying" "$stderr_output" "manual-action-required readiness proof should surface remediation guidance"
  assert_not_contains "tmux send-keys" "$stderr_output" "manual-action-required readiness proof should not imply blind approval"
  assert_not_contains "Allow? (y/n)" "$stderr_output" "manual-action-required readiness proof should not claim auto-approved prompt handling"

  rm -rf "$temp_dir"
}

run_daemon_helper_client_precondition_test() {
  local temp_dir remote_home remote_bin_dir state_dir stdout_file stderr_file log_file
  temp_dir=$(mktemp -d)
  remote_home="$temp_dir/remote-home"
  remote_bin_dir="$remote_home/.local/bin"
  state_dir="$temp_dir/state"
  stdout_file="$temp_dir/stdout.log"
  stderr_file="$temp_dir/stderr.log"
  log_file="$temp_dir/ssh-args.log"

  mkdir -p "$remote_bin_dir" "$state_dir"

  cat >"$remote_bin_dir/zjctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ $# -eq 1 && $1 == "--help" ]]; then
  exit 0
fi
if [[ $# -eq 3 && $1 == "panes" && $2 == "ls" && $3 == "--json" ]]; then
  printf 'helper client is not attached yet\n' >&2
  exit 70
fi
printf 'unexpected zjctl args: %s\n' "$*" >&2
exit 64
EOF
  chmod +x "$remote_bin_dir/zjctl"

  cat >"$remote_bin_dir/zellij" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
  chmod +x "$remote_bin_dir/zellij"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >>"$log_file"
printf -- '---\n' >>"$log_file"
remote_command=${!#}
env -i PATH=${WRAPPER_TEST_REMOTE_PATH:?} HOME=${WRAPPER_TEST_REMOTE_HOME:?} bash -c "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  if PATH="$temp_dir:$PATH" \
    WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
    WRAPPER_TEST_REMOTE_HOME="$remote_home" \
    WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
    mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
      --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
      --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
      --pretty zellij-discover --session-name aws --target aws >"$stdout_file" 2>"$stderr_file"; then
    printf 'expected helper-client precondition proof to fail\n' >&2
    exit 1
  fi

  assert_eq "" "$(<"$stdout_file")" "helper-client precondition proof should not emit stdout"

  local stderr_output
  stderr_output=$(<"$stderr_file")
  local ssh_args
  ssh_args=$(<"$log_file")
  assert_contains 'PLUGIN_NOT_READY' "$stderr_output" "helper-client precondition proof should surface plugin-not-ready class"
  assert_contains '-oBatchMode=yes' "$ssh_args" "helper-client precondition proof should run through the daemon-backed ssh path"
  assert_contains 'aws' "$ssh_args" "helper-client precondition proof should target the requested ssh alias"

  rm -rf "$temp_dir"
}

run_daemon_alias_only_selection_flow_test() {
  local temp_dir remote_home remote_bin_dir state_dir discover_file attach_file capture_file log_file env_log_file
  temp_dir=$(mktemp -d)
  remote_home="$temp_dir/remote-home"
  remote_bin_dir="$remote_home/.local/bin"
  state_dir="$temp_dir/state"
  discover_file="$temp_dir/discover.json"
  attach_file="$temp_dir/attach.json"
  capture_file="$temp_dir/capture.json"
  log_file="$temp_dir/ssh-args.log"
  env_log_file="$temp_dir/daemon-env.log"

  mkdir -p "$remote_bin_dir" "$state_dir"

  cat >"$remote_bin_dir/zjctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/daemon-env.log
printf 'ARGS=%s\n' "$*" >>"$log_file"
printf 'PATH=%s\n' "$PATH" >>"$log_file"
printf 'SESSION=%s\n' "${ZELLIJ_SESSION_NAME-}" >>"$log_file"
if [[ $# -eq 1 && $1 == "--help" ]]; then
  exit 0
fi
if [[ $# -eq 3 && $1 == "panes" && $2 == "ls" && $3 == "--json" ]]; then
  cat <<'JSON'
[{"id":"terminal:3","tab_name":"ops","title":"shell","command":"fish","focused":true}]
JSON
  exit 0
fi
if [[ $# -ge 4 && $1 == "pane" && $2 == "capture" && $3 == "--pane" ]]; then
  printf 'remote alias proof\n'
  exit 0
fi
printf 'unexpected zjctl args: %s\n' "$*" >&2
exit 64
EOF
  chmod +x "$remote_bin_dir/zjctl"

  cat >"$remote_bin_dir/zellij" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
  chmod +x "$remote_bin_dir/zellij"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >>"$log_file"
printf -- '---\n' >>"$log_file"
remote_command=${!#}
env -i WRAPPER_TEST_CAPTURE_DIR=${WRAPPER_TEST_CAPTURE_DIR:?} PATH=${WRAPPER_TEST_REMOTE_PATH:?} HOME=${WRAPPER_TEST_REMOTE_HOME:?} bash -c "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-discover --session-name gpu --target aws >"$discover_file"

  assert_contains '"target_id": "ssh:aws"' "$(<"$discover_file")" "daemon discover should canonicalize alias-only target ids"
  assert_contains '"selector": "id:terminal:3"' "$(<"$discover_file")" "daemon discover should return remote pane selector"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-attach --session-name gpu --selector id:terminal:3 --target aws >"$attach_file"

  assert_contains '"target_id": "ssh:aws"' "$(<"$attach_file")" "daemon attach should preserve canonical remote target ids"
  local handle
  handle=$(json_string_field handle "$attach_file")

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-capture --handle "$handle" --mode full >"$capture_file"

  assert_contains '"content": "remote alias proof' "$(<"$capture_file")" "daemon follow-up capture should route by persisted binding without target"

  local ssh_args env_log
  ssh_args=$(<"$log_file")
  env_log=$(<"$env_log_file")
  assert_contains '-oBatchMode=yes' "$ssh_args" "daemon remote flow should force batch mode over ssh"
  assert_contains 'aws' "$ssh_args" "daemon remote flow should use the provided ssh alias"
  assert_contains "PATH=$remote_bin_dir:/usr/local/bin:/usr/bin:/bin" "$env_log" "daemon remote flow should prepend ~/.local/bin before running remote zjctl"
  assert_contains 'SESSION=gpu' "$env_log" "daemon remote flow should pass the remote session name through follow-up zjctl commands"

  rm -rf "$temp_dir"
}

run_daemon_remote_busy_spawn_recovery_test() {
  local temp_dir remote_home remote_bin_dir state_dir spawn_file capture_file list_file log_file env_log_file
  temp_dir=$(mktemp -d)
  remote_home="$temp_dir/remote-home"
  remote_bin_dir="$remote_home/.local/bin"
  state_dir="$temp_dir/state"
  spawn_file="$temp_dir/spawn.json"
  capture_file="$temp_dir/capture.json"
  list_file="$temp_dir/list.json"
  log_file="$temp_dir/ssh-args.log"
  env_log_file="$temp_dir/daemon-env.log"

  mkdir -p "$remote_bin_dir" "$state_dir"

  cat >"$remote_bin_dir/zjctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/daemon-env.log
printf 'ARGS=%s\n' "$*" >>"$log_file"
printf 'PATH=%s\n' "$PATH" >>"$log_file"
printf 'SESSION=%s\n' "${ZELLIJ_SESSION_NAME-}" >>"$log_file"

if [[ $# -eq 1 && $1 == "--help" ]]; then
  exit 0
fi
if [[ $# -eq 3 && $1 == "panes" && $2 == "ls" && $3 == "--json" ]]; then
  cat <<'JSON'
[{"id":"terminal:9","tab_name":"ops","title":"repro-busy","command":"bash -lc printf busy\\n","focused":true}]
JSON
  exit 0
fi
if [[ $# -ge 2 && $1 == "pane" && $2 == "launch" ]]; then
  printf 'id:terminal:9\n'
  exit 0
fi
if [[ $# -ge 2 && $1 == "pane" && $2 == "wait-idle" ]]; then
  printf 'timed out after 30.0s\n' >&2
  exit 70
fi
if [[ $# -ge 4 && $1 == "pane" && $2 == "capture" && $3 == "--pane" ]]; then
  printf 'remote busy recovered\n'
  exit 0
fi
printf 'unexpected zjctl args: %s\n' "$*" >&2
exit 64
EOF
  chmod +x "$remote_bin_dir/zjctl"

  cat >"$remote_bin_dir/zellij" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
  chmod +x "$remote_bin_dir/zellij"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >>"$log_file"
printf -- '---\n' >>"$log_file"
remote_command=${!#}
env -i WRAPPER_TEST_CAPTURE_DIR=${WRAPPER_TEST_CAPTURE_DIR:?} PATH=${WRAPPER_TEST_REMOTE_PATH:?} HOME=${WRAPPER_TEST_REMOTE_HOME:?} bash -c "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-spawn --session-name gpu --spawn-target existing_tab --command "bash -lc 'printf busy'" --title repro-busy --wait-ready --target aws >"$spawn_file"

  assert_contains '"status": "busy"' "$(<"$spawn_file")" "remote spawn should degrade to a recoverable busy handle"
  assert_contains '"target_id": "ssh:aws"' "$(<"$spawn_file")" "remote spawn should preserve canonical target ownership"
  assert_contains '"_daemon": {' "$(<"$spawn_file")" "remote spawn should include daemon freshness metadata"
  local handle
  handle=$(json_string_field handle "$spawn_file")

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-capture --handle "$handle" --mode full >"$capture_file"

  assert_contains 'remote busy recovered' "$(<"$capture_file")" "follow-up capture should recover the busy remote handle without needing target"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-list --target aws --session-name gpu >"$list_file"

  assert_contains '"status": "ready"' "$(<"$list_file")" "later list should show the recovered handle as ready after revalidation"
  assert_contains '"target_id": "ssh:aws"' "$(<"$list_file")" "list should keep canonical remote target ids after recovery"

  local ssh_args env_log
  ssh_args=$(<"$log_file")
  env_log=$(<"$env_log_file")
  assert_contains '-oBatchMode=yes' "$ssh_args" "busy remote spawn recovery should still use daemon-backed ssh routing"
  assert_contains 'aws' "$ssh_args" "busy remote spawn recovery should use the configured ssh alias"
  assert_contains 'SESSION=gpu' "$env_log" "busy remote spawn recovery should keep the remote session bound across follow-up calls"

  rm -rf "$temp_dir"
}

run_daemon_discover_preview_degradation_test() {
  local temp_dir remote_home remote_bin_dir state_dir discover_file log_file env_log_file
  temp_dir=$(mktemp -d)
  remote_home="$temp_dir/remote-home"
  remote_bin_dir="$remote_home/.local/bin"
  state_dir="$temp_dir/state"
  discover_file="$temp_dir/discover.json"
  log_file="$temp_dir/ssh-args.log"
  env_log_file="$temp_dir/daemon-env.log"

  mkdir -p "$remote_bin_dir" "$state_dir"

  cat >"$remote_bin_dir/zjctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/daemon-env.log
printf 'ARGS=%s\n' "$*" >>"$log_file"
printf 'PATH=%s\n' "$PATH" >>"$log_file"
printf 'SESSION=%s\n' "${ZELLIJ_SESSION_NAME-}" >>"$log_file"

if [[ $# -eq 1 && $1 == "--help" ]]; then
  exit 0
fi
if [[ $# -eq 3 && $1 == "panes" && $2 == "ls" && $3 == "--json" ]]; then
  cat <<'JSON'
[{"id":"terminal:3","tab_name":"ops","title":"degraded","command":"tail -f app.log","focused":false},{"id":"terminal:4","tab_name":"ops","title":"healthy","command":"bash","focused":true}]
JSON
  exit 0
fi
if [[ $# -ge 4 && $1 == "pane" && $2 == "capture" && $3 == "--pane" && $4 == "id:terminal:3" ]]; then
  printf 'capture backend failed\n' >&2
  exit 70
fi
if [[ $# -ge 4 && $1 == "pane" && $2 == "capture" && $3 == "--pane" && $4 == "id:terminal:4" ]]; then
  printf 'healthy line 1\nhealthy line 2\nhealthy line 3\n'
  exit 0
fi
printf 'unexpected zjctl args: %s\n' "$*" >&2
exit 64
EOF
  chmod +x "$remote_bin_dir/zjctl"

  cat >"$remote_bin_dir/zellij" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exit 0
EOF
  chmod +x "$remote_bin_dir/zellij"

  cat >"$temp_dir/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log_file=${WRAPPER_TEST_CAPTURE_DIR:?}/ssh-args.log
for arg in "$@"; do
  printf '%s\n' "$arg"
done >>"$log_file"
printf -- '---\n' >>"$log_file"
remote_command=${!#}
env -i WRAPPER_TEST_CAPTURE_DIR=${WRAPPER_TEST_CAPTURE_DIR:?} PATH=${WRAPPER_TEST_REMOTE_PATH:?} HOME=${WRAPPER_TEST_REMOTE_HOME:?} bash -c "$remote_command"
EOF
  chmod +x "$temp_dir/ssh"

  PATH="$temp_dir:$PATH" \
  WRAPPER_TEST_CAPTURE_DIR="$temp_dir" \
  WRAPPER_TEST_REMOTE_HOME="$remote_home" \
  WRAPPER_TEST_REMOTE_PATH="/usr/local/bin:/usr/bin:/bin" \
  mcp2cli --mcp-stdio "$(daemon_stdio_command)" \
    --env "ZELLIJ_MCP_STATE_DIR=$state_dir" \
    --env 'ZELLIJ_MCP_TARGETS={"defaults":{"remote_zjctl_bin":"zjctl","remote_zellij_bin":"zellij"}}' \
    --pretty zellij-discover --session-name gpu --tab-name ops --include-preview --target aws >"$discover_file"

  local discover_output
  discover_output=$(<"$discover_file")
  assert_contains '"selector": "id:terminal:3"' "$discover_output" "discover should keep metadata for the degraded preview target"
  assert_contains '"title": "degraded"' "$discover_output" "discover should preserve degraded target metadata"
  assert_contains '"selector": "id:terminal:4"' "$discover_output" "discover should keep the healthy preview target"
  assert_contains 'healthy line 1\nhealthy line 2\nhealthy line 3\n' "$discover_output" "discover should still include preview text for healthy targets"
  assert_contains '"_daemon": {' "$discover_output" "discover should include daemon freshness metadata"

  local ssh_args env_log
  ssh_args=$(<"$log_file")
  env_log=$(<"$env_log_file")
  assert_contains '-oBatchMode=yes' "$ssh_args" "preview degradation flow should still run through daemon-backed ssh"
  assert_contains 'aws' "$ssh_args" "preview degradation flow should use the configured ssh alias"
  assert_contains "PATH=$remote_bin_dir:/usr/local/bin:/usr/bin:/bin" "$env_log" "preview degradation flow should normalize the remote PATH before probing"

  rm -rf "$temp_dir"
}

run_missing_alias_test
run_wrapper_exec_test
run_alias_only_path_discovery_test
run_manual_action_required_readiness_test
run_daemon_helper_client_precondition_test
run_daemon_alias_only_selection_flow_test
run_daemon_remote_busy_spawn_recovery_test
run_daemon_discover_preview_degradation_test
printf 'ok\n'
