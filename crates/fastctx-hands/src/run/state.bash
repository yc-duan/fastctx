#!/bin/bash
# Compile shell state in a clean shell, so script-owned readonly names and
# functions cannot collide with the compiler's own working variables.
set -euo pipefail
IFS= read -r -d '' login
IFS= read -r -d '' carried_vars
IFS= read -r -d '' carried_funcs
IFS= read -r -d '' carried_opts
IFS= read -r -d '' flags
vars=${login%%$'\036'*}
rest=${login#*$'\036'}
funcs=${rest%%$'\036'*}
opts=${rest#*$'\036'}

if [ "$1" = names ]; then
    if [ "$2" = variables ]; then
        baseline=$vars
        carried=$carried_vars
    else
        baseline=$funcs
        carried=$carried_funcs
    fi
    while IFS= read -r name; do
        case $'\n'"$carried"$'\n' in
            *$'\n'"$name"$'\n'*) ;;
            *) case $'\n'"$baseline"$'\n' in *$'\n'"$name"$'\n'*) continue ;; esac ;;
        esac
        if [ "$2" = variables ]; then
            case $name in
                __fastctx_login | __fastctx_flags | __fastctx_carried_vars | __fastctx_carried_funcs | \
                __fastctx_carried_opts | __fastctx_incoming | __fastctx_state_helper | __fastctx_program | \
                BASH | BASH_ALIASES | BASH_ARGC | BASH_ARGV | BASH_ARGV0 | BASH_CMDS | BASH_COMMAND | \
                BASH_EXECUTION_STRING | BASH_LINENO | BASH_REMATCH | BASH_SOURCE | BASH_SUBSHELL | \
                BASH_VERSINFO | BASH_VERSION | BASHOPTS | BASHPID | COLUMNS | COMP_CWORD | COMP_KEY | \
                COMP_LINE | COMP_POINT | COMP_TYPE | COMP_WORDBREAKS | COMP_WORDS | DIRSTACK | EPOCHREALTIME | \
                EPOCHSECONDS | EUID | FUNCNAME | GROUPS | HISTCMD | HOSTNAME | HOSTTYPE | IFS | LINENO | LINES | \
                MACHTYPE | OLDPWD | OPTARG | OPTERR | OPTIND | OSTYPE | PIPESTATUS | PPID | PWD | RANDOM | \
                READLINE_LINE | READLINE_POINT | READLINE_MARK | REPLY | SECONDS | SHELLOPTS | SHLVL | SRANDOM | UID | _)
                    continue ;;
            esac
        else
            case $name in __fastctx_state | __fastctx_select | __fastctx_on | __fastctx_on_pipe | on | edit | write | apply_patch) continue ;; esac
        fi
        printf '%q ' "$name"
    done
    exit 0
fi

IFS= read -r -d '' declarations
IFS= read -r -d '' definitions
IFS= read -r -d '' attributes
# The chosen functions' names come first, quoted as this helper quoted them,
# then every function's attributes; only the chosen ones' are kept.
chosen=${attributes%%$'\n'*}
listing=${attributes#*$'\n'}
attributes=
eval "set -- $chosen"
while IFS= read -r attribute; do
    for name in "$@"; do
        if [ "${attribute#declare -* }" = "$name" ]; then
            attributes+="$attribute"$'\n'
            break
        fi
    done
done <<<"$listing"
printf 'builtin shopt -s extglob || return 255\n'
vars_sent=
while IFS= read -r declaration; do
    [ -n "$declaration" ] || continue
    name=${declaration#declare -* }
    name=${name%%=*}
    vars_sent+="$name"$'\n'
    printf 'if [[ $(builtin declare -p -- %q 2>/dev/null) == "declare -n"* ]]; then builtin unset -n -- %q || return 255; fi\n' "$name" "$name"
    printf 'builtin unset -v -- %q || return 255\n' "$name"
    variable_attrs=${declaration#declare -}
    variable_attrs=${variable_attrs%% *}
    case $variable_attrs in
        *A*) printf 'builtin declare -A -- %q || return 255\n' "$name" ;;
        *a*) printf 'builtin declare -a -- %q || return 255\n' "$name" ;;
    esac
    assignment=${declaration#declare -* }
    # Before 4.4, bash prints an array's value as one quoted word,
    # name='([0]="a")', which assigned as it is makes a string; the word's
    # own value is the compound assignment.
    case $variable_attrs:$assignment in
        *[aA]*:*=\'\(*\)\') value=${assignment#*=}; eval "value=$value"; assignment=${assignment%%=*}=$value ;;
    esac
    if [[ $assignment == *=* ]]; then printf '%s || return 255\n' "$assignment"; fi
    if [ "$variable_attrs" = - ]; then
        printf 'builtin declare -- %q || return 255\n' "$name"
    else
        printf 'builtin declare -%s -- %q || return 255\n' "$variable_attrs" "$name"
    fi
done <<<"$declarations"
funcs_sent=
while IFS= read -r attribute; do
    [ -n "$attribute" ] || continue
    name=${attribute#declare -* }
    funcs_sent+="$name"$'\n'
    printf 'builtin unset -f -- %q || return 255\n' "$name"
done <<<"$attributes"
printf '%s\n' "$definitions"
while IFS= read -r attribute; do
    [ -n "$attribute" ] || continue
    function_attrs=${attribute#declare -}
    function_attrs=${function_attrs%% *}
    if [ "$function_attrs" != f ]; then printf 'builtin %s || return 255\n' "$attribute"; fi
done <<<"$attributes"
opts_sent=
changes=
while IFS= read -r option; do
    case $option in
        'set +o errexit' | 'set -o errexit')
            case $flags in *e*) option='set -o errexit' ;; *) option='set +o errexit' ;; esac ;;
    esac
    case $'\n'"$carried_opts"$'\n' in
        *$'\n'"${option##* }"$'\n'*) ;;
        *) case $'\n'"$opts"$'\n' in
            *$'\n'"$option"$'\n'*)
                case $option in 'shopt -u extglob') changes+="builtin $option || return 255"$'\n' ;; esac
                continue ;;
        esac ;;
    esac
    opts_sent+="${option##* }"$'\n'
    changes+="builtin $option || return 255"$'\n'
done
printf '__fastctx_carried_vars=%q\n__fastctx_carried_funcs=%q\n__fastctx_carried_opts=%q\n' "$vars_sent" "$funcs_sent" "$opts_sent"
printf '%s' "$changes"
printf 'builtin return 0\n'
