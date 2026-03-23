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
  assert_contains "'/remote home'\\''s/.local/bin'" "$(<"$log_file")" "remote paths should be shell-quoted"
  assert_contains "'sess'\\''one'" "$(<"$log_file")" "session name should be shell-quoted"
  assert_contains "'helper'\\''one'" "$(<"$log_file")" "helper session should be shell-quoted"

  rm -rf "$temp_dir"
}

run_missing_alias_test
run_help_test
run_quote_contract_test
printf 'ok\n'
