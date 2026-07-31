#compdef zotero

# zsh completion for zotero. `open`/`edit` suggest paper keys by shelling out
# to `zotero list`, which is instant once the local cache is warm.

_zotero_paper_keys() {
    local -a keys
    keys=(${(f)"$(zotero list 2>/dev/null)"})
    _describe -t paper-keys 'paper key' keys
}

_zotero() {
    local curcontext="$curcontext" state line
    typeset -A opt_args

    _arguments -C \
        '1: :->command' \
        '*:: :->args'

    case $state in
        command)
            local -a subcmds
            subcmds=(
                'list:List items in your library'
                'search:Search your library, printing matching keys'
                'add:Add an item to your library from a DOI'
                'open:Open an item'\''s PDF (or its URL/DOI)'
                'edit:Edit an item'\''s metadata in \$EDITOR'
                'config:Configure your Zotero API credentials'
                'completions:Print a shell completion script'
            )
            _describe 'zotero command' subcmds
            ;;
        args)
            case $line[1] in
                open|edit)
                    _arguments '1:paper key:_zotero_paper_keys'
                    ;;
                list|search)
                    _arguments \
                        '--subcollection=[Only items in this subcollection]:name:' \
                        '--tag=[Only items with this tag]:tag:_zotero_paper_keys'
                    ;;
                completions)
                    _arguments '1:shell:(zsh)'
                    ;;
            esac
            ;;
    esac
}

_zotero "$@"
