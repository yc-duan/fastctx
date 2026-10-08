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
__fastctx_login=$(compgen -v; printf '\036'; compgen -A function; printf '\036'; shopt -p; set +o)

# State `on` brought from the machine that sent this script: restored after
# the snapshot, so a further `on` carries it on.
if [ -n "${FASTCTX_ON_STATE-}" ]; then
    . "$FASTCTX_ON_STATE"
    unset FASTCTX_ON_STATE
fi

# Writes the script's own variables, functions and changed options, as bash
# reads them back.
__fastctx_state() {
    local IFS=$'\n' __fastctx_n __fastctx_attrs
    local __fastctx_vars=${__fastctx_login%%$'\036'*}
    local __fastctx_rest=${__fastctx_login#*$'\036'}
    local __fastctx_funcs=${__fastctx_rest%%$'\036'*}
    local __fastctx_opts=${__fastctx_rest#*$'\036'}
    for __fastctx_n in $(compgen -v); do
        case $'\n'"$__fastctx_vars"$'\n' in *$'\n'"$__fastctx_n"$'\n'*) continue ;; esac
        case $__fastctx_n in
            __fastctx_* | BASH | BASH_* | BASHOPTS | BASHPID | COLUMNS | COMP_* | DIRSTACK | EPOCHREALTIME | \
            EPOCHSECONDS | EUID | FUNCNAME | GROUPS | HISTCMD | HOSTNAME | HOSTTYPE | IFS | LINENO | LINES | \
            MACHTYPE | OLDPWD | OPTARG | OPTERR | OPTIND | OSTYPE | PIPESTATUS | PPID | PWD | RANDOM | \
            READLINE_* | REPLY | SECONDS | SHELLOPTS | SHLVL | SRANDOM | UID | _ | FASTCTX_*)
                continue ;;
        esac
        declare -p -- "$__fastctx_n" 2>/dev/null
    done
    for __fastctx_n in $(compgen -A function); do
        case $'\n'"$__fastctx_funcs"$'\n' in *$'\n'"$__fastctx_n"$'\n'*) continue ;; esac
        case $__fastctx_n in __fastctx_* | on | edit | write | apply_patch) continue ;; esac
        declare -f -- "$__fastctx_n"
        __fastctx_attrs=$(declare -F -p -- "$__fastctx_n" 2>/dev/null)
        case $__fastctx_attrs in "declare -fx "*) printf 'declare -fx -- %s\n' "$__fastctx_n" ;; esac
    done
    for __fastctx_n in $(shopt -p; set +o); do
        case $'\n'"$__fastctx_opts"$'\n' in *$'\n'"$__fastctx_n"$'\n'*) continue ;; esac
        printf '%s\n' "$__fastctx_n"
    done
}

# on MACHINE [COMMAND [ARGUMENT...]]: this part of the script, run on
# another machine (world-run.md, 去别的机器). With a command, its words are
# sent as they are; without one, standard input is the script.
on() {
    if [ $# -eq 0 ]; then
        fastctx on
        return
    fi
    local __fastctx_file="${FASTCTX_SCRATCH:?the World did not start this shell}/on-$BASHPID-$RANDOM$RANDOM.state"
    {
        # A bash the script started has no prelude, and so no record of the
        # login environment: there it carries nothing rather than too much.
        if declare -F __fastctx_state >/dev/null; then
            __fastctx_state
        fi
        if [ $# -gt 1 ]; then
            local -a __fastctx_argv=("${@:2}")
            declare -p __fastctx_argv
        fi
    } >"$__fastctx_file" || return 255
    if [ $# -gt 1 ]; then
        fastctx on --state "$__fastctx_file" --command -- "$1"
    else
        fastctx on --state "$__fastctx_file" -- "$1"
    fi
}

# The World's ways of writing files (world-write.md). Functions, because
# these names belong elsewhere too: Windows has an editor called `edit`,
# Linux a `write` that messages other users.
edit() { fastctx edit "$@"; }
write() { fastctx write "$@"; }
apply_patch() { fastctx apply-patch "$@"; }

export -f on edit write apply_patch
