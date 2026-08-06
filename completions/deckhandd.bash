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

# Complete a control-socket path. On Unix it is a filesystem path (a Windows pipe name is free-form);
# offering files + directories is the useful default either way.
_deckhand_socket() {
    COMPREPLY=( $(compgen -f -- "$cur") $(compgen -d -- "$cur") )
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
-k --socket -p --prevent-sleep -s --start -v --verbose -h --help -V --version"

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
        -k|--socket)
            _deckhand_socket
            return
            ;;
    esac

    # Otherwise: complete options (this tool has no subcommands or positionals).
    COMPREPLY=( $(compgen -W "$opts" -- "$cur") )
}

# --- deckhandctl -------------------------------------------------------------

# Commands can be chained (deckhandctl runs them in sequence), so completion tracks where we are in
# the chain: after a command that takes an argument we complete the argument; otherwise (at a command
# boundary) we complete the next command keyword.
_deckhandctl() {
    local cur prev words cword i tok need started
    _deckhand_get_words

    # Value for the global --socket option (must precede the commands).
    case $prev in
        -k|--socket)
            _deckhand_socket
            return
            ;;
    esac

    local cmds="status list-devices input output main fallback globals \
start stop shutdown monitor"

    # Walk the words before the cursor: `need` holds a command still awaiting its argument (else empty
    # = at a command boundary); `started` is set once any command word is seen (global flags are only
    # valid before that).
    need=""
    started=""
    for (( i=1; i < cword; i++ )); do
        tok=${words[i]}
        case $tok in
            -k|--socket) (( i++ )); continue ;;   # skip the socket value
            -*) continue ;;                        # other global flags (-h/-V)
        esac
        started=1
        if [[ -n $need ]]; then
            need=""                                # this word is the awaited argument
        else
            case $tok in
                input|output|main|fallback|globals) need=$tok ;;  # awaits an argument
                *) need="" ;;                                     # 0-arg command
            esac
        fi
    done

    # Complete based on the chain state at the cursor.
    if [[ -n $need ]]; then
        case $need in
            input) _deckhand_input ;;
            output) COMPREPLY=( $(compgen -W "$_deckhand_output_specs" -- "$cur") ) ;;
            main|fallback|globals) _deckhand_ron_files ;;
        esac
    elif [[ -z $started ]]; then
        # At the very start: commands plus the global flags.
        COMPREPLY=( $(compgen -W "$cmds -k --socket -h --help -V --version" -- "$cur") )
    else
        # Between commands in a chain: just the next command keyword.
        COMPREPLY=( $(compgen -W "$cmds" -- "$cur") )
    fi
}

complete -F _deckhandd deckhandd
complete -F _deckhandctl deckhandctl
