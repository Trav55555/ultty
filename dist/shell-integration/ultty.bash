# Ultty shell integration for bash 4.4 or later.
#
# Marks each prompt, command line, command output and exit status with OSC
# 133, and reports the working directory with OSC 7 at every prompt, so `ulttyctl last-command`, `ulttyctl wait-for --command-done` and the
# `command_started` / `command_finished` events work. Other terminals that
# understand OSC 133 use the same marks; the rest ignore them.
#
# Ultty loads it by itself in the shells it starts (`shell_integration` in
# the config). To load it by hand instead, for a shell Ultty did not start,
# source it at the end of ~/.bashrc:
#   source /usr/share/ultty/shell-integration/ultty.bash
# Sourcing it twice, or by hand in an Ultty shell, marks once.

[[ $- == *i* ]] || return 0
[[ -n ${__ultty_integration-} ]] && return 0
__ultty_integration=1

# D: the last command finished. Runs first in PROMPT_COMMAND so it sees the
# command's status, and passes that status on to the rest.
__ultty_command_done() {
    local status=$?
    printf '\e]133;D;%s\a' "$status"
    return "$status"
}

# OSC 7: the working directory, as file://host/path with the path
# percent-encoded byte by byte.
__ultty_report_cwd() {
    local LC_ALL=C path=$PWD encoded= ch i
    if [[ $path =~ ^[-A-Za-z0-9_./~]*$ ]]; then
        encoded=$path
    else
        for ((i = 0; i < ${#path}; i++)); do
            ch=${path:i:1}
            case $ch in
                [-A-Za-z0-9_./~]) encoded+=$ch ;;
                *) printf -v ch '%%%02X' "'$ch"; encoded+=$ch ;;
            esac
        done
    fi
    printf '\e]7;file://%s%s\a' "${HOSTNAME-}" "$encoded"
}

# A and B around the prompt. Runs last in PROMPT_COMMAND because prompt
# themes rebuild PS1 there.
__ultty_mark_prompt() {
    local status=$?
    __ultty_report_cwd
    [[ $PS1 == *'133;A'* ]] || PS1='\[\e]133;A\a\]'"$PS1"
    [[ $PS1 == *'133;B'* ]] || PS1="$PS1"'\[\e]133;B\a\]'
    return "$status"
}

# C: output starts. Bash prints PS0 after reading a command line, before
# running it.
PS0="${PS0-}"$'\e]133;C\a'

if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(__ultty_command_done "${PROMPT_COMMAND[@]}" __ultty_mark_prompt)
else
    # Others' commands often end in `;` (pyenv's does); drop it before
    # adding ours, since `;;` is a syntax error.
    __ultty_rest=${PROMPT_COMMAND-}
    while [[ $__ultty_rest =~ [[:space:]\;]$ ]]; do
        __ultty_rest=${__ultty_rest%?}
    done
    PROMPT_COMMAND="__ultty_command_done${__ultty_rest:+; $__ultty_rest}; __ultty_mark_prompt"
    unset __ultty_rest
fi
