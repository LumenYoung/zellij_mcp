function __zellij_mcp_run_b64
    set -l preview 0
    if test (count $argv) -gt 0; and test "$argv[1]" = "-p"
        set preview 1
        set -e argv[1]
    end

    if test (count $argv) -lt 2
        printf '%s\n' 'usage: __zellij_mcp_run_b64 [-p] <interaction-id> <payload>' >&2
        return 2
    end

    set -l interaction_id $argv[1]
    set -l payload $argv[2]
    set -l decoded (printf '%s' "$payload" | base64 --decode 2>/dev/null)
    if test $status -ne 0
        printf '%s\n' 'zellij-mcp wrapper failed to decode payload' >&2
        return 2
    end

    if test $preview -eq 1
        printf '# zellij-mcp preview:\n%s\n' "$decoded"
        return 0
    end

    printf '# zellij-mcp command:\n%s\n' "$decoded"
    printf '__ZELLIJ_MCP_INTERACTION__:start:%s\n' "$interaction_id"
    eval $decoded
    set -l __zellij_mcp_status $status
    printf '\n__ZELLIJ_MCP_INTERACTION__:end:%s:%s\n' "$interaction_id" "$__zellij_mcp_status"
    return $__zellij_mcp_status
end
