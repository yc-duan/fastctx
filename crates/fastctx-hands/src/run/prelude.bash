# The World's prelude. `run` names this file in BASH_ENV, so bash reads it
# after the login profile and before the script (world-run.md). It leaves
# the script's line numbers as they are, and nothing of it reaches the
# programs the script starts except the exported functions below.

unset BASH_ENV

# A profile may have rewritten PATH (Git's and Debian's both do): the
# World's programs come first again.
if [ -n "${FASTCTX_BIN-}" ]; then
    PATH="$FASTCTX_BIN:$PATH"
fi
unset FASTCTX_BIN

# What the login environment already holds, so that `on` carries only what
# the script itself makes (world-run.md, 心智随行).
__fastctx_flags=$-
__fastctx_login=$(
    compgen -v; printf '\036'; compgen -A function; printf '\036'; shopt -p
    # Command substitution normally clears errexit. Read the caller's
    # setting, not that implementation detail of the snapshot.
    case $__fastctx_flags in *e*) set -e ;; esac
    set +o
)

# Writes the script's own variables, functions and changed options, as bash
# reads them back.
__fastctx_state() {
    local IFS=$'\n' __fastctx_n __fastctx_attrs __fastctx_flags=$-
    local __fastctx_vars=${__fastctx_login%%$'\036'*}
    local __fastctx_rest=${__fastctx_login#*$'\036'}
    local __fastctx_funcs=${__fastctx_rest%%$'\036'*}
    local __fastctx_opts=${__fastctx_rest#*$'\036'}
    local __fastctx_vars_sent= __fastctx_funcs_sent= __fastctx_opts_sent= __fastctx_changes=
    # Function definitions may contain extended patterns. They must be
    # parsed with extglob enabled even when the script turned it off again.
    printf 'shopt -s extglob || return 255\n'
    for __fastctx_n in $(compgen -v); do
        # A name brought by a previous hop belongs to the script even if
        # this machine's login environment happened to have the same name.
        case $'\n'"${__fastctx_carried_vars-}"$'\n' in
            *$'\n'"$__fastctx_n"$'\n'*) ;;
            *) case $'\n'"$__fastctx_vars"$'\n' in *$'\n'"$__fastctx_n"$'\n'*) continue ;; esac ;;
        esac
        case $__fastctx_n in
            __fastctx_* | BASH | BASH_* | BASHOPTS | BASHPID | COLUMNS | COMP_* | DIRSTACK | EPOCHREALTIME | \
            EPOCHSECONDS | EUID | FUNCNAME | GROUPS | HISTCMD | HOSTNAME | HOSTTYPE | IFS | LINENO | LINES | \
            MACHTYPE | OLDPWD | OPTARG | OPTERR | OPTIND | OSTYPE | PIPESTATUS | PPID | PWD | RANDOM | \
            READLINE_* | REPLY | SECONDS | SHELLOPTS | SHLVL | SRANDOM | UID | _ | FASTCTX_*)
                continue ;;
        esac
        __fastctx_vars_sent+="$__fastctx_n"$'\n'
        # Discard target-side attributes as well as the old value. Removing
        # a nameref must not unset the variable it points to.
        printf 'if [[ $(declare -p -- %q 2>/dev/null) == "declare -n"* ]]; then unset -n -- %q || return 255; fi\n' "$__fastctx_n" "$__fastctx_n"
        printf 'unset -v -- %q || return 255\n' "$__fastctx_n"
        printf '%s || return 255\n' "$(declare -p -- "$__fastctx_n")"
    done
    for __fastctx_n in $(compgen -A function); do
        case $'\n'"${__fastctx_carried_funcs-}"$'\n' in
            *$'\n'"$__fastctx_n"$'\n'*) ;;
            *) case $'\n'"$__fastctx_funcs"$'\n' in *$'\n'"$__fastctx_n"$'\n'*) continue ;; esac ;;
        esac
        case $__fastctx_n in __fastctx_* | on | edit | write | apply_patch) continue ;; esac
        __fastctx_funcs_sent+="$__fastctx_n"$'\n'
        printf 'unset -f -- %q || return 255\n' "$__fastctx_n"
        declare -f -- "$__fastctx_n"
        __fastctx_attrs=$(declare -F -p -- "$__fastctx_n" 2>/dev/null)
        __fastctx_attrs=${__fastctx_attrs#declare -}
        __fastctx_attrs=${__fastctx_attrs%% *}
        if [ "$__fastctx_attrs" != f ]; then
            printf 'declare -%s -- %q || return 255\n' "$__fastctx_attrs" "$__fastctx_n"
        fi
    done
    for __fastctx_n in $(shopt -p; case $__fastctx_flags in *e*) set -e ;; esac; set +o); do
        case $'\n'"${__fastctx_carried_opts-}"$'\n' in
            *$'\n'"${__fastctx_n##* }"$'\n'*) ;;
            *) case $'\n'"$__fastctx_opts"$'\n' in
                *$'\n'"$__fastctx_n"$'\n'*)
                    # Undo the parser setting above even when it was not
                    # a script change. It must not become a carried option.
                    case $__fastctx_n in 'shopt -u extglob') __fastctx_changes+="$__fastctx_n || return 255"$'\n' ;; esac
                    continue ;;
            esac ;;
        esac
        __fastctx_opts_sent+="${__fastctx_n##* }"$'\n'
        __fastctx_changes+="$__fastctx_n || return 255"$'\n'
    done
    printf '__fastctx_carried_vars=%q\n__fastctx_carried_funcs=%q\n__fastctx_carried_opts=%q\n' \
        "$__fastctx_vars_sent" "$__fastctx_funcs_sent" "$__fastctx_opts_sent"
    printf '%s' "$__fastctx_changes"
    printf 'return 0\n'
}

# on MACHINE [COMMAND [ARGUMENT...]]: this part of the script, run on
# another machine (world-run.md, 去别的机器). With a command, its words are
# sent as they are; without one, standard input is the script.
on() {
    case $#:${1-} in
        0:) fastctx on; return ;;
        1:-h | 1:--help) fastctx on --help; return ;;
    esac
    # A whole script as one word is how ssh is written; here one word is the
    # name of a program to run, so it is refused before anything is sent.
    if [ $# -eq 2 ] && [[ $2 == *[[:space:]\;\|\&\<\>\$\`\(\)]* ]]; then
        printf '%s\n' "on: '$2' is one word, the name of a program to run on $1. For a script, give it on standard input: on $1 <<'EOF' ... EOF" >&2
        return 255
    fi
    local __fastctx_file="${FASTCTX_SCRATCH:?on works only inside a script run by the run tool}/on-$BASHPID-$RANDOM$RANDOM"
    # The follower ends with this shell. Git Bash names the shell's native
    # Windows process, which a native follower can wait on, in /proc.
    local __fastctx_shell=$BASHPID
    if [ -r "/proc/$BASHPID/winpid" ]; then
        read -r __fastctx_shell <"/proc/$BASHPID/winpid" || return 255
    fi
    # A bash the script started has no prelude, and so no record of the
    # login environment: there it carries nothing rather than too much.
    if declare -F __fastctx_state >/dev/null; then
        __fastctx_state >"$__fastctx_file.state" || return 255
    else
        : >"$__fastctx_file.state" || return 255
    fi
    if [ $# -gt 1 ]; then
        # The words, quoted by bash itself, become the command line bash
        # reads there: it reads back exactly these words.
        printf '%q ' "${@:2}" >"$__fastctx_file.command" || return 255
        fastctx on --shell-parent "$__fastctx_shell" --state "$__fastctx_file.state" --command "$__fastctx_file.command" -- "$1"
    else
        fastctx on --shell-parent "$__fastctx_shell" --state "$__fastctx_file.state" -- "$1"
    fi
    # Keep the shell alive as the follower's parent, even when this is the
    # script's last command. On Linux the follower dies with this shell.
    return $?
}

# The World's ways of writing files (world-write.md). Functions, because
# these names belong elsewhere too: Windows has an editor called `edit`,
# Linux a `write` that messages other users.
edit() { fastctx edit "$@"; }
write() { fastctx write "$@"; }
apply_patch() { fastctx apply-patch "$@"; }

export -f on edit write apply_patch

# Restore last: options such as verbose and nounset govern the requested
# script, not the implementation of the World's prelude. A failed restore
# must never execute a script with only part of its state.
__fastctx_incoming=${FASTCTX_ON_STATE-}
unset FASTCTX_ON_STATE
if [ -n "$__fastctx_incoming" ]; then
    . "$__fastctx_incoming" || exit 255
fi
