# Bash completion for loct (loctree CLI)
# Add to ~/.bashrc: source /path/to/loct.bash
# Or: cp loct.bash /etc/bash_completion.d/loct

_loct_completions() {
    local cur prev commands global_opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    # Main subcommands
    commands="auto agent scan tree slice find dead unused cycles commands events info lint report help query diff crowd tagmap twins suppress routes dist coverage impact focus hotspots layoutmap"

    # Global options
    global_opts="--json --quiet --verbose --color --library-mode --python-library --py-root --help --help-full --help-legacy --version"

    # Command-specific options
    case "${COMP_WORDS[1]}" in
        slice)
            local opts="--consumers --no-consumers --json --root --rescan --depth"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        find)
            local opts="--json --root --dead-only --symbol-only --semantic-only"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        dead|unused)
            local opts="--json --root --confidence --full"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        cycles)
            local opts="--json --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        commands)
            local opts="--json --missing --unused"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        events)
            local opts="--json"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        twins)
            local opts="--json --dead-only --include-suppressed --include-tests"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        crowd)
            local opts="--json --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        tagmap)
            local opts="--json --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        focus)
            local opts="--json --consumers --depth --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        hotspots)
            local opts="--json --leaves --coupling --min --limit --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        layoutmap)
            local opts="--json --zindex-only --sticky-only --grid-only --min-zindex --exclude --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        suppress)
            local opts="--list --clear --file --reason"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        scan)
            local opts="--json --full-scan --watch --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        lint)
            local opts="--fail --sarif --json --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        report)
            local opts="--serve --port --graph --json --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        query)
            local opts="who-imports where-symbol component-of"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        diff)
            local opts="--since --json"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        dist)
            local opts="--json"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
        impact)
            local opts="--json --depth --root"
            COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
            return 0
            ;;
    esac

    # Handle --color values
    if [[ "${prev}" == "--color" ]]; then
        COMPREPLY=( $(compgen -W "auto always never" -- ${cur}) )
        return 0
    fi

    # Handle --confidence values
    if [[ "${prev}" == "--confidence" ]]; then
        COMPREPLY=( $(compgen -W "normal high" -- ${cur}) )
        return 0
    fi

    # Default: show subcommands or global options
    if [[ ${cur} == -* ]]; then
        COMPREPLY=( $(compgen -W "${global_opts}" -- ${cur}) )
    elif [[ ${COMP_CWORD} -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "${commands}" -- ${cur}) )
    else
        # File/directory completion
        COMPREPLY=( $(compgen -f -- ${cur}) )
    fi

    return 0
}

complete -F _loct_completions loct
complete -F _loct_completions loctree
