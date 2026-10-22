# bash completion for deckhand-hidhide (and its `hidhide` short alias)
#
# Installed to "${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions" by install.sh
# as deckhand-hidhide.bash, plus a hidhide.bash symlink to it (the dir is lazily loaded by command
# name, so each command name needs its own file).

# Id hints only - any hex id is accepted. Valve VID + the Steam Controller / Deck PIDs
# (Gordon 1102/1106/1142, Neptune 1205, Triton 1302/1303/1304).
_hidhide_vendors="28de"
_hidhide_products="1102 1106 1142 1205 1302 1303 1304"

# Wrap bash-completion's word helpers (renamed across versions), with a plain fallback.
_hidhide_get_words() {
    if declare -F _comp_get_words >/dev/null 2>&1; then
        _comp_get_words cur prev words cword
    elif declare -F _get_comp_words_by_ref >/dev/null 2>&1; then
        _get_comp_words_by_ref cur prev words cword
    else
        cur=${COMP_WORDS[COMP_CWORD]}
        prev=${COMP_WORDS[COMP_CWORD-1]}
        words=("${COMP_WORDS[@]}")
        cword=$COMP_CWORD
    fi
}

_hidhide() {
    local cur prev words cword
    _hidhide_get_words

    local opts="-v --vendor -p --product --hidraw-only -h --help -V --version"

    # Value for an option that takes one (hex id hints).
    case $prev in
        -v|--vendor)  COMPREPLY=( $(compgen -W "$_hidhide_vendors" -- "$cur") ); return ;;
        -p|--product) COMPREPLY=( $(compgen -W "$_hidhide_products" -- "$cur") ); return ;;
    esac

    # Find where the wrapped command begins: the first word that is neither an option nor an
    # option's value, or the word right after a literal `--`.
    local i cmd=0
    for (( i=1; i < cword; i++ )); do
        case ${words[i]} in
            --) cmd=$(( i + 1 )); break ;;
            -v|--vendor|-p|--product) (( i++ )) ;;   # skip the option's value
            -*) ;;                                    # a flag (e.g. --hidraw-only)
            *) cmd=$i; break ;;                       # the command name
        esac
    done

    # Inside the wrapped command -> delegate to its own completion (command name, then its args).
    if (( cmd > 0 && cword >= cmd )); then
        if declare -F _command_offset >/dev/null 2>&1; then
            _command_offset "$cmd"
        elif (( cword == cmd )); then
            COMPREPLY=( $(compgen -c -- "$cur") )
        else
            COMPREPLY=( $(compgen -f -- "$cur") )
            compopt -o filenames 2>/dev/null
        fi
        return
    fi

    # Still in deckhand-hidhide's own words: an option, or a bare word starting the command.
    if [[ $cur == -* ]]; then
        COMPREPLY=( $(compgen -W "$opts" -- "$cur") )
    else
        COMPREPLY=( $(compgen -c -- "$cur") )
    fi
}

complete -F _hidhide deckhand-hidhide hidhide
