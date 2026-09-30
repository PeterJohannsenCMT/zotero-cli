#compdef zotero

# zsh completion for zotero. Paper/collection completions shell out to
# `zotero list`/`zotero collections list`, which are instant once the local
# cache is warm.

_zotero_paper_keys() {
    local -a keys
    keys=("${(@f)$(zotero list 2>/dev/null | awk '{
        key = $1
        sub(/^[^[:space:]]+[[:space:]]*/, "")
        print key ":" $0
    }')}")
    _describe -t paper-keys 'paper key' keys
}

_zotero_collection_names() {
    local -a collections
    collections=(${(f)"$(zotero collections list 2>/dev/null | sed 's/^[[:space:]]*//')"})
    _describe -t collection-names 'collection' collections
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
                'collections:List or search your subcollections'
                'add:Add an item from a DOI or enter metadata manually'
                'open:Open an item'\''s PDF (or its URL/DOI)'
                'show:Show an item'\''s details'
                'edit:Edit an item'\''s metadata in \$EDITOR'
                'remove:Remove an item from your Zotero library'
                'attach-pdf:Attach a PDF file to an item'
                'fetch-pdf:Look up and attach a PDF for an existing item'
                'export:Export items as BibTeX'
                'sync:Force a sync with the Zotero API'
                'check-citation-keys:Check for gaps in citation-key suffixes'
                'config:Configure your Zotero API credentials'
                'completions:Print a shell completion script'
            )
            _describe 'zotero command' subcmds
            ;;
        args)
            case $line[1] in
                open|show|edit|remove|fetch-pdf)
                    _arguments '1:paper key:_zotero_paper_keys'
                    ;;
                list|search)
                    _arguments \
                        '--subcollection=[Only items in this subcollection]:collection:_zotero_collection_names' \
                        '--tag=[Only items with this tag]:tag:_zotero_paper_keys'
                    ;;
                add)
                    _arguments \
                        '--manual[Enter item metadata by hand in $EDITOR]' \
                        '--type=[Zotero item type for manual mode]:item type:(journalArticle book bookSection webpage report thesis dataset conferencePaper)' \
                        '--nopdf[Do not try to download and attach a PDF]' \
                        '1:DOI or manual:(manual)'
                    ;;
                attach-pdf)
                    _arguments \
                        '1:paper key:_zotero_paper_keys' \
                        '2:PDF file:_files -g "*.pdf(-.)"'
                    ;;
                export)
                    _arguments \
                        '--bibitem[Export as LaTeX bibitem entries instead of BibTeX]' \
                        '--no-revtex-journals[Preserve Zotero journal strings instead of REVTeX/apsrev macros]' \
                        '--subcollection=[Export items in this subcollection]:collection:_zotero_collection_names' \
                        '--tag=[Export items with this tag]:tag:_zotero_paper_keys' \
                        '*:paper key:_zotero_paper_keys'
                    ;;
                collections)
                    local -a collections_subcmds
                    collections_subcmds=(
                        'list:List all subcollections as an indented tree'
                        'search:Search subcollections by name'
                        'use:Set the default subcollection for list/search/export'
                        'current:Show the current default subcollection'
                        'clear:Return to the library top level by default'
                        'edit:Rename a subcollection'
                        'move-item:Replace an item'\''s collection membership with one subcollection'
                        'add-item:Add an item to a subcollection, keeping existing memberships'
                        'remove-item:Remove an item from a subcollection'
                    )
                    if (( CURRENT == 2 )); then
                        _describe 'collections command' collections_subcmds
                    else
                        case $line[2] in
                            search)
                                _arguments '1:query:'
                                ;;
                            use)
                                _arguments '1:collection:_zotero_collection_names'
                                ;;
                            edit)
                                _arguments \
                                    '1:collection:_zotero_collection_names' \
                                    '2:new name:'
                                ;;
                            move-item|add-item|remove-item)
                                _arguments \
                                    '1:paper key:_zotero_paper_keys' \
                                    '2:collection:_zotero_collection_names'
                                ;;
                        esac
                    fi
                    ;;
                completions)
                    _arguments '1:shell:(zsh)'
                    ;;
            esac
            ;;
    esac
}

_zotero "$@"
