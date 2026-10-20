# The World's prelude. `run` names this file in BASH_ENV, so bash reads it
# after the login profile and before the script (world-run.md). It leaves
# the script's line numbers as they are. Child shells inherit its helpers
# and provenance names; login values are never packed into an on state.

unset BASH_ENV
__fastctx_state_helper="${BASH_SOURCE[0]%/*}/state.bash"

# A profile may have rewritten PATH (Git's and Debian's both do): the
# World's programs come first again.
#
# The functions below call the World's program by its path. A function or
# alias named `fastctx` or `command` cannot stand in for a path, and a plain
# call keeps every exit-status rule bash has; `builtin command` does not:
# a failure under it ends a `set -e` script even inside `if`, `||` or `!`.
__fastctx_program=
if [ -n "${FASTCTX_BIN-}" ]; then
    PATH="$FASTCTX_BIN:$PATH"
    __fastctx_program=$FASTCTX_BIN/fastctx
fi
unset FASTCTX_BIN

# What the login environment already holds, so that `on` carries only what
# the script itself makes (world-run.md, 心智随行).
__fastctx_carried_vars=
__fastctx_carried_funcs=
__fastctx_carried_opts=
__fastctx_flags=$-
__fastctx_login=$(
    compgen -v; printf '\036'; compgen -A function; printf '\036'; shopt -p
    # Command substitution normally clears errexit. Read the caller's
    # setting, not that implementation detail of the snapshot. The pattern
    # opens with its own parenthesis: bash 3.2 ends `$(` at a bare `)`.
    case $__fastctx_flags in (*e*) set -e ;; esac
    set +o
)

# Only names are sent to the selector. Login values never enter a packet.
__fastctx_select() {
    {
        builtin printf '%s\0' "$__fastctx_login" "$__fastctx_carried_vars" "$__fastctx_carried_funcs" "$__fastctx_carried_opts" "$-"
        if [ "$1" = variables ]; then builtin compgen -v; else builtin compgen -A function; fi
    } | /usr/bin/env -i /bin/bash --noprofile --norc "$__fastctx_state_helper" names "$1"
}

# Positional parameters keep the collector out of the script's variable
# namespace, including names the script made readonly.
__fastctx_state() {
    builtin set -- "${1-$-}" "$(__fastctx_select variables)" "$(__fastctx_select functions)"
    {
        builtin printf '%s\0' "$__fastctx_login" "$__fastctx_carried_vars" "$__fastctx_carried_funcs" "$__fastctx_carried_opts" "$1"
        if [ -n "$2" ]; then builtin eval "builtin declare -p -- $2" || builtin return 255; fi
        builtin printf '\0'
        if [ -n "$3" ]; then builtin eval "builtin declare -f -- $3" || builtin return 255; fi
        builtin printf '\0'
        # The chosen names, then every function's attributes: bash 3.2 lists
        # them only all at once. The helper keeps the chosen ones'.
        if [ -n "$3" ]; then builtin printf '%s\n' "$3"; builtin declare -F; fi
        builtin printf '\0'
        builtin shopt -p
        builtin set +o
    } | /usr/bin/env -i /bin/bash --noprofile --norc "$__fastctx_state_helper" state
    builtin set -- "${PIPESTATUS[@]}"
    [ "$1" = 0 ] && [ "$2" = 0 ] || builtin return 255
}

# Each call that can fail is followed by `|| builtin return`: a failure
# inside a function would otherwise fire an inherited ERR trap (set -E) a
# second time, and once even under `!`, which a program typed directly never
# does. The function then fails with the program's status, where bash
# applies its rules once, as for the program.
on() { __fastctx_on 0 - "$@" || builtin return; }

# The first two arguments describe an on-to-on pipe; the remainder are the
# script's words. No collector local can shadow one of its variables.
__fastctx_on() {
    case $#:${3-} in
        2:) "$__fastctx_program" on || builtin return; builtin return ;;
        3:-h | 3:--help) "$__fastctx_program" on --help || builtin return; builtin return ;;
    esac
    if [ $# -eq 4 ] && [[ $4 == *[[:space:]\;\|\&\<\>\$\`\(\)]* ]]; then
        builtin printf '%s\n' "on: '$4' is one word, the name of a program to run on $3. For a script, give it on standard input: on $3 <<'EOF' ... EOF" >&2
        builtin return 255
    fi
    # This shell's own process: BASHPID from bash 4 on; in bash 3.2, which
    # macOS keeps, the parent of a program it starts.
    builtin set -- "${BASHPID:-$(builtin exec /usr/bin/env -i /bin/sh -c 'echo "$PPID"')}" "$@"
    builtin set -- "${FASTCTX_JOB_TEMP:?on works only inside a script run by the run tool}/on-$1-$RANDOM$RANDOM" "$@"
    __fastctx_state "$-" >"$1.state" || builtin return 255
    if [ -r "/proc/$2/winpid" ]; then
        builtin set -- "$1" "$(/usr/bin/cat "/proc/$2/winpid")" "${@:3}"
    fi
    if [ $# -gt 5 ]; then
        builtin printf '%q ' "${@:6}" >"$1.command" || builtin return 255
        builtin set -- "${@:1:5}" --command "$1.command"
    fi
    if [ "$3" = 1 ] && [ -p /dev/stdin ]; then builtin set -- "$@" --pipe-in; fi
    if [ "$4" != - ] && [ -p /dev/stdout ]; then builtin set -- "$@" --pipe-to "$4"; fi
    "$__fastctx_program" on "${@:6}" --shell-parent "$2" --state "$1.state" -- "$5" || builtin return
}

# An `on` the script pipes straight from or into another, as the run tool
# marks it before bash reads the script (world-run.md, 数据跟着命令走): the
# first word is 1 when its input comes from an `on`, the second the machine
# of the `on` its output goes to, or -.
__fastctx_on_pipe() {
    __fastctx_on "$@" || builtin return
}

# The World's ways of writing files (world-write.md). Functions, because
# these names belong elsewhere too: Windows has an editor called `edit`,
# Linux a `write` that messages other users.
edit() { "$__fastctx_program" edit "$@" || builtin return; }
write() { "$__fastctx_program" write "$@" || builtin return; }
apply_patch() { "$__fastctx_program" apply-patch "$@" || builtin return; }

export -f on __fastctx_on __fastctx_on_pipe __fastctx_state __fastctx_select edit write apply_patch
export __fastctx_program __fastctx_state_helper __fastctx_login __fastctx_flags __fastctx_carried_vars __fastctx_carried_funcs __fastctx_carried_opts

# Restore last: options such as verbose and nounset govern the requested
# script, not the implementation of the World's prelude. A failed restore
# must never execute a script with only part of its state.
__fastctx_incoming=${FASTCTX_ON_STATE-}
unset FASTCTX_ON_STATE
if [ -n "$__fastctx_incoming" ]; then
    . "$__fastctx_incoming" || exit 255
fi
