# bash completion for deckhandd and deckhandctl (deckhand)
#
# Installed to "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions" by install.sh
# as deckhandd.bash with a deckhandctl.bash symlink (the dir is lazily loaded by command name).

# --- shared data -------------------------------------------------------------

# Input source spec: keywords plus the common device ids. A device id is
# shape:transport:interface:serial (e.g. gordon:dongle:1:) — serial is free-form, so we offer
# the usual prefixes and let the user finish the serial (usually empty).
#   shape     := gordon | neptune
#   transport := dongle | wired
#   interface := 1-9
#   serial    := anything
_deckhand_input_specs="auto dongle wired \
gordon:wired:1: \
gordon:dongle:1: gordon:dongle:2: gordon:dongle:3: gordon:dongle:4: \
neptune:wired:1:"

# Output sink spec.
_deckhand_output_specs="local"

# --- colon handling ----------------------------------------------------------
# Device ids are colon-delimited, but bash breaks words on ':' by default. These wrap
# bash-completion's helpers (renamed across versions) to (a) rebuild cur/prev/words with ':'
# kept in-word — so `-i <id>` is still recognized and cur holds the whole id — and (b) trim the
# already-typed prefix after adding candidates, so ids complete cleanly instead of duplicating.

_deckhand_get_words() {
    if declare -F _comp_get_words >/dev/null 2>&1; then
        _comp_get_words -n : cur prev words cword
    elif declare -F _get_comp_words_by_ref >/dev/null 2>&1; then
        _get_comp_words_by_ref -n : cur prev words cword
    else
        cur=${COMP_WORDS[COMP_CWORD]}
        prev=${COMP_WORDS[COMP_CWORD-1]}
        words=("${COMP_WORDS[@]}")
        cword=$COMP_CWORD
    fi
}

_deckhand_ltrim_colon() {
    if declare -F _comp_ltrim_colon_completions >/dev/null 2>&1; then
        _comp_ltrim_colon_completions "$cur"
    elif declare -F __ltrim_colon_completions >/dev/null 2>&1; then
        __ltrim_colon_completions "$cur"
    fi
}

# --- shared helpers ----------------------------------------------------------

# Complete RON profile files (plus directories to descend into).
_deckhand_ron_files() {
    COMPREPLY=( $(compgen -f -X '!*.ron' -- "$cur") $(compgen -d -- "$cur") )
    compopt -o filenames 2>/dev/null
}

# Complete an input spec (keywords + common device ids), colons handled.
_deckhand_input() {
    COMPREPLY=( $(compgen -W "$_deckhand_input_specs" -- "$cur") )
    _deckhand_ltrim_colon
}

# --- deckhandd ---------------------------------------------------------------

_deckhandd() {
    local cur prev words cword
    _deckhand_get_words

    local opts="-m --main -f --fallback -g --globals -i --input -o --output \
-s --start -v --verbose -h --help -V --version"

    # Value completion for the option that takes one.
    case $prev in
        -m|--main|-f|--fallback|-g|--globals)
            _deckhand_ron_files
            return
            ;;
        -i|--input)
            _deckhand_input
            return
            ;;
        -o|--output)
            COMPREPLY=( $(compgen -W "$_deckhand_output_specs" -- "$cur") )
            return
            ;;
    esac

    # Otherwise: complete options (this tool has no subcommands or positionals).
    COMPREPLY=( $(compgen -W "$opts" -- "$cur") )
}

# --- deckhandctl -------------------------------------------------------------

_deckhandctl() {
    local cur prev words cword cmd i
    _deckhand_get_words

    local cmds="status list-devices input output main fallback globals \
start stop shutdown monitor help"

    # Find the subcommand (first non-option word after argv[0]).
    cmd=""
    for (( i=1; i < cword; i++ )); do
        case ${words[i]} in
            -*) ;;
            *) cmd=${words[i]}; break ;;
        esac
    done

    # No subcommand yet → complete the subcommand (or top-level flags).
    if [[ -z $cmd ]]; then
        COMPREPLY=( $(compgen -W "$cmds -h --help -V --version" -- "$cur") )
        return
    fi

    # Argument completion per subcommand.
    case $cmd in
        input)
            _deckhand_input
            ;;
        output)
            COMPREPLY=( $(compgen -W "$_deckhand_output_specs" -- "$cur") )
            ;;
        main|fallback|globals)
            _deckhand_ron_files
            ;;
        help)
            COMPREPLY=( $(compgen -W "$cmds" -- "$cur") )
            ;;
        *)
            # status/list-devices/start/stop/shutdown/monitor take no arguments.
            COMPREPLY=()
            ;;
    esac
}

complete -F _deckhandd deckhandd
complete -F _deckhandctl deckhandctl
